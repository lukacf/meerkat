//! Console schedules (`meta/schedule/*` on `/console/rpc`): deferred console
//! sends a Console caller sets up for an agent, judged at every fire as that
//! caller's own send.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use meerkat::{
    HostRunnableInvocation, HostRunnableName, MemoryScheduleStore, ScheduleId, ScheduleService,
};
use meerkat_mob::SpawnMemberSpec;
use serde_json::{Value, json};

use super::*;
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleFrameStatus, ConsoleTimelineQuery,
};
use crate::console_schedules::{
    CONSOLE_SCHEDULE_RUNNABLE, ConsoleScheduleDoorSlot, ConsoleScheduleSender,
    ConsoleScheduledSend, with_console_schedule_runnable,
};
use crate::runtime::AuthenticatedConsoleHuman;
use crate::schedule_wiring::ScheduleFiringHostBinding;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const ISSUER: &str = "https://issuer.example.test";
const ERIN: &str = "erin@example.test";
const FRANK: &str = "frank@example.test";
const VIEWER: &str = "viewer@example.test";
const NOBODY: &str = "nobody@example.test";
const ROBOT: &str = "svc:robot";

/// A console runtime with the agents `alpha` and `beta`, the console
/// aggregator over it, and, when `firing` is given, the schedule service a
/// gateway's schedule host attaches, drained or not as `firing` says.
struct Fixture {
    _temp_dir: tempfile::TempDir,
    /// The composition's public owner, for a fixture whose agents take
    /// console sends ([`delivering_fixture`]).
    owner: Option<crate::unified_runtime::UnifiedRuntime>,
    runtime: MobRuntime,
    aggregator: MetaConsoleAggregator,
    events: ConsoleEventStore,
    service: ScheduleService,
    /// The identity runtime of an identity-first fixture
    /// ([`identity_first_fixture`]).
    identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
}

impl Fixture {
    async fn stop(self) {
        match self.owner {
            Some(owner) => {
                let _ = owner.shutdown().await;
            }
            None => {
                let _ = self.runtime.handle().stop().await;
            }
        }
    }
}

async fn spawn_agents(runtime: &MobRuntime, role: &str) -> TestResult {
    for alias in ["alpha", "beta"] {
        runtime
            .handle()
            .spawn_spec(SpawnMemberSpec::from_wire(
                role.to_string(),
                alias.to_string(),
                None,
                None,
                None,
            ))
            .await?;
    }
    Ok(())
}

fn fixture_over(
    temp_dir: tempfile::TempDir,
    owner: Option<crate::unified_runtime::UnifiedRuntime>,
    runtime: MobRuntime,
    identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
    firing: Option<ScheduleFiringHostBinding>,
) -> Fixture {
    let aggregator = MetaConsoleAggregator::in_memory();
    let events = ConsoleEventStore::new();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        identity_runtime.clone(),
        events.clone(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
    if let Some(firing) = firing {
        runtime.console_schedules().attach(service.clone(), firing);
    }
    Fixture {
        _temp_dir: temp_dir,
        owner,
        runtime,
        aggregator,
        events,
        service,
        identity_runtime,
    }
}

/// A light console runtime for the methods' answers: its agents are
/// listed, and nothing is sent to them.
async fn fixture(
    mob_id: &str,
    firing: Option<ScheduleFiringHostBinding>,
) -> Result<Fixture, Box<dyn std::error::Error + Send + Sync>> {
    let (temp_dir, runtime) = super::tests::build_empty_console_test_runtime(mob_id).await?;
    spawn_agents(&runtime, "worker").await?;
    Ok(fixture_over(temp_dir, None, runtime, None, firing))
}

/// A console runtime whose agents take console sends: a persistent
/// composition, as a gateway runs one, with a schedule service whose firing
/// host is bound.
async fn delivering_fixture(
    mob_id: &str,
) -> Result<Fixture, Box<dyn std::error::Error + Send + Sync>> {
    let temp_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let owner = crate::unified_runtime::UnifiedRuntimeBuilder::default()
        .definition(meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"
[profiles.agent]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "autonomous_host"
[profiles.agent.tools]
comms = true
"#
        ))?)
        .persistent_state(temp_dir.path())
        .comms(true)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .map_err(|error| error.to_string())?;
    let runtime = owner.mob_runtime().clone();
    spawn_agents(&runtime, "agent").await?;
    Ok(fixture_over(
        temp_dir,
        Some(owner),
        runtime,
        None,
        Some(ScheduleFiringHostBinding::external_driver()),
    ))
}

/// A console runtime whose agent `alpha` is a durable identity of an
/// identity-first runtime, as an identity-first gateway serves it: the
/// Console sends to it through the identity runtime (the production lane).
async fn identity_first_fixture(
    mob_id: &str,
) -> Result<Fixture, Box<dyn std::error::Error + Send + Sync>> {
    use crate::identity_first::{
        AgentAddressability, AgentIdentity, AgentRuntimeServices, DurabilityPolicy,
        DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
        LocalLeaseProvider, MobSessionBridge,
    };
    let temp_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let owner = crate::unified_runtime::UnifiedRuntimeBuilder::default()
        .definition(meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"
[profiles.agent]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "autonomous_host"
[profiles.agent.tools]
comms = true
"#
        ))?)
        .persistent_state(temp_dir.path())
        .comms(true)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .map_err(|error| error.to_string())?;
    let session_service = owner
        .mob_runtime()
        .session_service()
        .cloned()
        .ok_or("a persistent runtime has a session service")?;
    let bridge = Arc::new(MobSessionBridge::with_session_service(
        owner.mob_handle(),
        session_service,
    ));
    let identity_runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(
                LocalContinuityStore::in_memory().map_err(|error| format!("{error:?}"))?,
            ),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: format!("{mob_id}-instance"),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(owner.mob_handle())),
    );
    let spec = DurableAgentSpec {
        identity: AgentIdentity::parse("alpha").map_err(|error| format!("{error:?}"))?,
        profile: "agent".into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
        backend: None,
        binding: None,
        placement: None,
    };
    crate::identity_first::restore_flow(&identity_runtime, &[spec], None, None)
        .await
        .map_err(|error| format!("{error:?}"))?;
    let runtime = owner.mob_runtime().clone();
    Ok(fixture_over(
        temp_dir,
        Some(owner),
        runtime,
        Some(identity_runtime),
        Some(ScheduleFiringHostBinding::external_driver()),
    ))
}

/// Who calls: a verified Console human (its principal, with an OIDC subject
/// of its own), a service identity, or the open Console's shared account,
/// on a writable or read-only console, under an access policy or none.
#[derive(Clone, Copy)]
struct Caller<'a> {
    principal: Option<&'a str>,
    /// A service identity: an authenticated principal with no human.
    service: bool,
    read_only: bool,
    access: Option<&'a AccessController>,
}

impl<'a> Caller<'a> {
    fn human(principal: &'a str) -> Self {
        Self {
            principal: Some(principal),
            service: false,
            read_only: false,
            access: None,
        }
    }

    fn service(principal: &'a str) -> Self {
        Self {
            service: true,
            ..Self::human(principal)
        }
    }

    fn open_console() -> Self {
        Self {
            principal: None,
            service: false,
            read_only: false,
            access: None,
        }
    }

    fn under(self, access: &'a AccessController) -> Self {
        Self {
            access: Some(access),
            ..self
        }
    }

    /// The send scope `/console/send` resolves for this caller.
    fn scope(self) -> ConsoleSendScope {
        match self.principal {
            Some(principal) if self.service => ConsoleSendScope::Principal {
                principal: principal.to_string(),
                provider: "service_identity".to_string(),
                issuer: ISSUER.to_string(),
            },
            Some(principal) => ConsoleSendScope::Human {
                issuer: ISSUER.to_string(),
                subject: subject_of(principal),
            },
            None => ConsoleSendScope::Anonymous,
        }
    }
}

fn subject_of(principal: &str) -> String {
    format!("{principal}-sub")
}

fn request(method: &str, params: Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    }
}

/// `method` with `params` on the console route's dispatch, as `caller`.
async fn call(fixture: &Fixture, caller: Caller<'_>, method: &str, params: Value) -> Value {
    let view = caller
        .access
        .map(|controller| controller.view_for_subject(caller.principal));
    let governance = ConsoleSendGovernance {
        human: caller
            .principal
            .filter(|_| !caller.service)
            .map(|principal| AuthenticatedConsoleHuman::for_test(ISSUER, &subject_of(principal))),
        scope: caller.scope(),
        ..ConsoleSendGovernance::default()
    };
    Box::pin(handle_console_runtime_rpc_with_visibility(
        &fixture.runtime,
        None,
        None,
        None,
        None,
        Some(fixture.aggregator.clone()),
        fixture.identity_runtime.clone(),
        None,
        None,
        &AllowAllConsoleVisibilityPolicy,
        request(method, params),
        true,
        caller.read_only,
        caller.principal,
        caller.access,
        view.as_ref(),
        None,
        None,
        None,
        None,
        None,
        &governance,
        &crate::topology_control::TopologyApplyScope::Anonymous,
    ))
    .await
}

fn hourly() -> Value {
    json!({
        "type": "interval",
        "start_at_utc": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
        "every_seconds": 3600,
    })
}

fn create_params(identity: &str) -> Value {
    json!({
        "identity": identity,
        "content": "Post the morning summary.",
        "trigger": hourly(),
        "name": "Morning summary",
    })
}

fn error_of(answer: &Value) -> (Value, Value) {
    (
        answer["error"]["code"].clone(),
        answer["error"]["data"]["kind"].clone(),
    )
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

/// The console schedule methods `meta/capabilities` lists for the caller.
fn schedule_methods(capabilities: &Value) -> Vec<String> {
    capabilities["result"]["methods"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|method| method.starts_with("meta/schedule/") && *method != "meta/schedule/tools")
        .map(ToString::to_string)
        .collect()
}

/// The send a stored console schedule carries.
async fn stored_send(
    fixture: &Fixture,
    schedule_id: &str,
) -> Result<ConsoleScheduledSend, Box<dyn std::error::Error + Send + Sync>> {
    let stored = fixture
        .service
        .get(&ScheduleId::parse(schedule_id)?)
        .await?;
    ConsoleScheduledSend::of_target(&stored.target)
        .ok_or_else(|| "a console schedule's send".into())
}

/// An access rule `id` that gives `subject` `actions` on `agent`.
fn grant(id: &str, subject: &str, actions: &[&str], agent: &str) -> AccessRule {
    AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
        agents: vec![agent.to_string()],
        ..AccessRule::default()
    }
}

/// An enforced access policy over `rules`.
fn enforced(
    rules: Vec<AccessRule>,
) -> Result<AccessController, Box<dyn std::error::Error + Send + Sync>> {
    Ok(AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules,
        ..crate::access::AccessControlConfig::default()
    })?)
}

/// The list is a read of the agent and every write a send to it that also
/// needs `schedule.manage`; the writes are console mutations a read-only
/// console and a candidate launch refuse.
#[test]
fn console_schedules_are_gated_as_view_and_send_on_their_agent() {
    let params = json!({ "identity": "alpha", "schedule_id": "not-read-here" });
    assert_eq!(
        console_rpc_access_requirements("meta/schedule/list", &params),
        Some(vec![(ACTION_AGENT_VIEW, Some("alpha".to_string()))])
    );
    assert!(!is_console_mutating_rpc_method("meta/schedule/list"));
    assert!(!crate::rpc::CANDIDATE_LAUNCH_REFUSED_METHODS.contains(&"meta/schedule/list"));
    for method in schedules::SCHEDULE_WRITE_METHODS {
        assert_eq!(
            console_rpc_access_requirements(method, &params),
            Some(vec![
                (ACTION_AGENT_SEND, Some("alpha".to_string())),
                (ACTION_SCHEDULE_MANAGE, Some("alpha".to_string())),
            ]),
            "{method}"
        );
        assert!(is_console_mutating_rpc_method(method), "{method}");
        assert!(schedules::is_write(method), "{method}");
        assert!(
            crate::rpc::CANDIDATE_LAUNCH_REFUSED_METHODS.contains(method),
            "{method}"
        );
    }
}

/// The family is offered where the runtime's schedule host attached its
/// service: the list always, the writes where the console is writable and
/// a firing host drains the store, signed in or not.
#[tokio::test]
async fn the_family_is_offered_where_a_schedule_service_is_attached() -> TestResult {
    let every = strings(&[
        "meta/schedule/list",
        "meta/schedule/create",
        "meta/schedule/update",
        "meta/schedule/pause",
        "meta/schedule/resume",
        "meta/schedule/delete",
    ]);
    let list_only = strings(&["meta/schedule/list"]);

    let drained = fixture(
        "console-schedules-offered",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    for (caller, expected, why) in [
        (Caller::human(ERIN), &every, "a signed-in caller"),
        (Caller::open_console(), &every, "a console without sign-in"),
        (
            Caller {
                read_only: true,
                ..Caller::human(ERIN)
            },
            &list_only,
            "a read-only console",
        ),
    ] {
        let capabilities = call(&drained, caller, "meta/capabilities", json!({})).await;
        assert_eq!(&schedule_methods(&capabilities), expected, "{why}");
    }
    drained.stop().await;

    let undrained = fixture(
        "console-schedules-undrained",
        Some(ScheduleFiringHostBinding::pending_in_process_host()),
    )
    .await?;
    let capabilities = call(
        &undrained,
        Caller::human(ERIN),
        "meta/capabilities",
        json!({}),
    )
    .await;
    assert_eq!(
        schedule_methods(&capabilities),
        list_only,
        "no firing host drains the store"
    );
    let created = call(
        &undrained,
        Caller::human(ERIN),
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert_eq!(
        error_of(&created),
        (
            json!(crate::rpc::CAPABILITY_UNAVAILABLE_CODE),
            json!("schedule_host_unavailable")
        ),
        "{created:#?}"
    );
    undrained.stop().await;

    let absent = fixture("console-schedules-absent", None).await?;
    let capabilities = call(&absent, Caller::human(ERIN), "meta/capabilities", json!({})).await;
    assert_eq!(
        schedule_methods(&capabilities),
        Vec::<String>::new(),
        "a runtime with no schedule service offers none"
    );
    let listed = call(
        &absent,
        Caller::human(ERIN),
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    assert_eq!(listed["error"]["code"], json!(-32601), "{listed:#?}");
    absent.stop().await;
    Ok(())
}

/// A console schedule is created as a send to its agent by its caller,
/// listed under that agent only, taken over by another caller's change,
/// paused, taken back by a resume, and deleted.
#[tokio::test]
async fn a_console_schedule_lives_through_create_update_pause_resume_and_delete() -> TestResult {
    let fixture = fixture(
        "console-schedules-lifecycle",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let created = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    let item = created["result"].clone();
    let schedule_id = item["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();
    assert_eq!(item["identity"], json!("alpha"));
    assert_eq!(item["content"], json!("Post the morning summary."));
    assert_eq!(item["name"], json!("Morning summary"));
    assert_eq!(item["phase"], json!("active"));
    assert_eq!(item["sender"], json!(ERIN));
    assert_eq!(item["trigger"]["type"], json!("interval"));
    assert!(item["next_due_at_utc"].is_string(), "{item:#?}");

    // The schedule is Meta's durable schedule, whose target is the console's
    // runnable carrying the send: its agent and its caller's send scope.
    let stored = fixture
        .service
        .get(&ScheduleId::parse(&schedule_id)?)
        .await?;
    assert!(matches!(
        &stored.target,
        meerkat::TargetBinding::HostRunnable(binding)
            if binding.runnable.as_str() == CONSOLE_SCHEDULE_RUNNABLE
    ));
    let send = stored_send(&fixture, &schedule_id).await?;
    assert_eq!(send.identity, "alpha");
    assert_eq!(send.content, "Post the morning summary.");
    assert_eq!(
        send.sender,
        ConsoleScheduleSender::Human {
            principal: ERIN.to_string(),
            provider: "generic_oidc".to_string(),
            issuer: ISSUER.to_string(),
            subject: subject_of(ERIN),
        }
    );

    let listed = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    assert_eq!(listed["result"]["identity"], json!("alpha"), "{listed:#?}");
    assert_eq!(
        listed["result"]["schedules"]
            .as_array()
            .map(|schedules| schedules
                .iter()
                .map(|schedule| schedule["schedule_id"].clone())
                .collect::<Vec<_>>()),
        Some(vec![json!(schedule_id)])
    );
    let other = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/list",
        json!({ "identity": "beta" }),
    )
    .await;
    assert_eq!(other["result"]["schedules"], json!([]), "{other:#?}");

    // Another caller's change takes the schedule over; what it does not
    // name stays.
    let updated = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/update",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "content": "Post the evening summary.",
            "take_over_sender": true,
        }),
    )
    .await;
    assert!(updated["error"].is_null(), "{updated:#?}");
    assert_eq!(
        updated["result"]["content"],
        json!("Post the evening summary.")
    );
    assert_eq!(updated["result"]["name"], json!("Morning summary"));
    assert_eq!(updated["result"]["sender"], json!(FRANK));
    assert!(
        updated["result"]["revision"].as_u64() > item["revision"].as_u64(),
        "{updated:#?}"
    );
    assert_eq!(
        stored_send(&fixture, &schedule_id)
            .await?
            .sender
            .principal(),
        Some(FRANK)
    );

    let paused = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    assert!(paused["result"]["next_due_at_utc"].is_null());
    assert_eq!(
        paused["result"]["sender"],
        json!(FRANK),
        "a pause keeps the sender"
    );

    // A resume by another caller takes the schedule over too.
    let resumed = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/resume",
        json!({ "identity": "alpha", "schedule_id": schedule_id, "take_over_sender": true }),
    )
    .await;
    assert_eq!(resumed["result"]["phase"], json!("active"), "{resumed:#?}");
    assert_eq!(resumed["result"]["sender"], json!(ERIN));
    assert_eq!(
        stored_send(&fixture, &schedule_id)
            .await?
            .sender
            .principal(),
        Some(ERIN)
    );
    let again = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/resume",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(again["error"]["code"], json!(-32602), "{again:#?}");

    let deleted = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/delete",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(
        deleted["result"],
        json!({ "identity": "alpha", "schedule_id": schedule_id, "deleted": true }),
        "{deleted:#?}"
    );
    let gone = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(
        error_of(&gone),
        (json!(-32001), json!("schedule_not_found")),
        "{gone:#?}"
    );
    let listed = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    assert_eq!(listed["result"]["schedules"], json!([]), "{listed:#?}");
    fixture.stop().await;
    Ok(())
}

/// A console without sign-in schedules as its shared account, exactly as it
/// sends: every write passes the send's own gate, and the schedule records
/// the open Console's account as its sender, with no principal.
#[tokio::test]
async fn a_console_without_sign_in_schedules_as_its_shared_account() -> TestResult {
    let fixture = fixture(
        "console-schedules-open",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;

    // On the Console route: an open console resolves every caller to its
    // shared account, and the create is answered like any other write.
    let mut state = super::console_human_admission_tests::state();
    state.decisions = crate::runtime::RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            read_only: false,
            ..crate::ConsolePolicy::default()
        },
        None,
    );
    state.runtime = Some(fixture.runtime.clone());
    state.console_aggregator = Some(fixture.aggregator.clone());
    let response = console_rpc_handler(
        State(state),
        None,
        HeaderMap::new(),
        Uri::from_static("/console/rpc"),
        Json(json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": 1,
            "method": "meta/schedule/create",
            "params": create_params("alpha"),
        })),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let created: Value =
        serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await?)?;
    assert!(created["error"].is_null(), "{created:#?}");
    assert!(created["result"]["sender"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();
    assert_eq!(
        stored_send(&fixture, &schedule_id).await?.sender,
        ConsoleScheduleSender::Anonymous
    );

    let open = Caller::open_console();
    let updated = call(
        &fixture,
        open,
        "meta/schedule/update",
        json!({ "identity": "alpha", "schedule_id": schedule_id, "trigger": hourly() }),
    )
    .await;
    assert!(updated["error"].is_null(), "{updated:#?}");
    assert!(updated["result"]["sender"].is_null());
    let paused = call(
        &fixture,
        open,
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    let resumed = call(
        &fixture,
        open,
        "meta/schedule/resume",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(resumed["result"]["phase"], json!("active"), "{resumed:#?}");
    assert_eq!(
        stored_send(&fixture, &schedule_id).await?.sender,
        ConsoleScheduleSender::Anonymous
    );
    let deleted = call(
        &fixture,
        open,
        "meta/schedule/delete",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(deleted["result"]["deleted"], json!(true), "{deleted:#?}");

    // A read-only console keeps its own answer.
    let refused = call(
        &fixture,
        Caller {
            read_only: true,
            ..open
        },
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert_eq!(
        error_of(&refused),
        (json!(-32010), json!("read_only")),
        "{refused:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// Listing an agent's schedules needs `agent.view` on it and every write
/// `agent.send` on it, judged per agent: a viewer reads and changes nothing,
/// a sender schedules for its own agent only.
#[tokio::test]
async fn schedule_methods_judge_view_and_send_on_the_agent() -> TestResult {
    let fixture = fixture(
        "console-schedules-access",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let controller = enforced(vec![
        grant("viewer-views-alpha", VIEWER, &[ACTION_AGENT_VIEW], "alpha"),
        grant(
            "erin-drives-alpha",
            ERIN,
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
            "alpha",
        ),
    ])?;
    let viewer = Caller::human(VIEWER).under(&controller);
    let erin = Caller::human(ERIN).under(&controller);
    let nobody = Caller::human(NOBODY).under(&controller);
    let denied = |answer: &Value, action: &str| {
        assert_eq!(
            answer["error"]["code"],
            json!(ACCESS_DENIED_RPC_CODE),
            "{answer:#?}"
        );
        assert_eq!(
            answer["error"]["data"]["action"],
            json!(action),
            "{answer:#?}"
        );
    };

    let created = call(
        &fixture,
        erin,
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"].clone();
    denied(
        &call(
            &fixture,
            erin,
            "meta/schedule/create",
            create_params("beta"),
        )
        .await,
        ACTION_AGENT_SEND,
    );

    let listed = call(
        &fixture,
        viewer,
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    assert_eq!(
        listed["result"]["schedules"].as_array().map(Vec::len),
        Some(1),
        "{listed:#?}"
    );
    denied(
        &call(
            &fixture,
            viewer,
            "meta/schedule/create",
            create_params("alpha"),
        )
        .await,
        ACTION_AGENT_SEND,
    );
    for method in ["meta/schedule/pause", "meta/schedule/delete"] {
        denied(
            &call(
                &fixture,
                viewer,
                method,
                json!({ "identity": "alpha", "schedule_id": schedule_id }),
            )
            .await,
            ACTION_AGENT_SEND,
        );
    }
    denied(
        &call(
            &fixture,
            nobody,
            "meta/schedule/list",
            json!({ "identity": "alpha" }),
        )
        .await,
        ACTION_AGENT_VIEW,
    );

    // The capabilities list the family for both; each call is judged.
    let capabilities = call(&fixture, viewer, "meta/capabilities", json!({})).await;
    assert!(
        schedule_methods(&capabilities).contains(&"meta/schedule/create".to_string()),
        "{capabilities:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// Only a caller who may send to the agent sees who its schedules send as:
/// a viewer who holds `agent.view` alone reads the list with the sender and
/// its earlier senders withheld, while the senders' own writes and lists
/// show them.
#[tokio::test]
async fn only_a_caller_who_may_send_sees_who_a_schedule_sends_as() -> TestResult {
    let fixture = fixture(
        "console-schedules-sender-exposure",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let controller = enforced(vec![
        grant("viewer-views-alpha", VIEWER, &[ACTION_AGENT_VIEW], "alpha"),
        grant(
            "erin-drives-alpha",
            ERIN,
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
            "alpha",
        ),
        grant(
            "frank-drives-alpha",
            FRANK,
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
            "alpha",
        ),
    ])?;
    let erin = Caller::human(ERIN).under(&controller);
    let created = call(
        &fixture,
        erin,
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert_eq!(created["result"]["sender"], json!(ERIN), "{created:#?}");
    assert_eq!(created["result"]["sender_hidden"], json!(false));
    let taken = call(
        &fixture,
        Caller::human(FRANK).under(&controller),
        "meta/schedule/update",
        json!({
            "identity": "alpha",
            "schedule_id": created["result"]["schedule_id"],
            "content": "Post the morning summary.",
            "take_over_sender": true,
        }),
    )
    .await;
    assert_eq!(taken["result"]["sender"], json!(FRANK), "{taken:#?}");

    let viewed = call(
        &fixture,
        Caller::human(VIEWER).under(&controller),
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    let item = &viewed["result"]["schedules"][0];
    assert!(item["sender"].is_null(), "{viewed:#?}");
    assert_eq!(item["sender_hidden"], json!(true), "{viewed:#?}");
    assert_eq!(item["sender_history"], json!([]), "{viewed:#?}");
    assert_eq!(
        item["content"],
        json!("Post the morning summary."),
        "the rest of the schedule is listed"
    );

    let managed = call(
        &fixture,
        erin,
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    let item = &managed["result"]["schedules"][0];
    assert_eq!(item["sender"], json!(FRANK), "{managed:#?}");
    assert_eq!(item["sender_hidden"], json!(false));
    assert_eq!(
        item["sender_history"][0]["sender"],
        json!(ERIN),
        "{managed:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// The handler judges what the access table cannot: the agent must be one
/// this console knows and sends to, and a schedule id must name a console
/// schedule of that agent; anything else is answered alike, not found.
#[tokio::test]
async fn the_handler_judges_the_schedule_and_its_agent() -> TestResult {
    let fixture = fixture(
        "console-schedules-targets",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let erin = Caller::human(ERIN);

    let unknown = call(
        &fixture,
        erin,
        "meta/schedule/create",
        create_params("ghost"),
    )
    .await;
    assert_eq!(
        error_of(&unknown),
        (json!(-32001), json!("unknown_identity")),
        "{unknown:#?}"
    );
    for (params, why) in [
        (
            json!({ "identity": "alpha", "trigger": hourly() }),
            "no content",
        ),
        (
            json!({ "identity": "alpha", "content": "   ", "trigger": hourly() }),
            "blank content",
        ),
        (
            json!({ "identity": "alpha", "content": "Hello." }),
            "no trigger",
        ),
        (
            json!({ "identity": "alpha", "content": "Hello.", "trigger": { "type": "weekly" } }),
            "an unknown trigger",
        ),
        (
            json!({
                "identity": "alpha",
                "content": "Hello.",
                "trigger": {
                    "type": "interval",
                    "start_at_utc": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                    "every_seconds": 30,
                },
            }),
            "an interval under a minute",
        ),
        (
            json!({ "content": "Hello.", "trigger": hourly() }),
            "no agent",
        ),
    ] {
        let refused = call(&fixture, erin, "meta/schedule/create", params).await;
        assert_eq!(
            refused["error"]["code"],
            json!(-32602),
            "{why}: {refused:#?}"
        );
    }

    let created = call(
        &fixture,
        erin,
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"].clone();

    // A schedule an agent authored with its own tools is not the console's.
    let authored = fixture
        .service
        .create(meerkat::CreateScheduleRequest {
            name: None,
            description: None,
            trigger: serde_json::from_value(hourly())?,
            target: serde_json::from_value(
                json!({ "target_kind": "host_runnable", "runnable": "nightly-report" }),
            )?,
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await?;

    for (params, why) in [
        (
            json!({ "identity": "beta", "schedule_id": schedule_id }),
            "another agent's schedule",
        ),
        (
            json!({ "identity": "alpha", "schedule_id": authored.schedule_id.to_string() }),
            "a schedule the console did not make",
        ),
        (
            json!({ "identity": "alpha", "schedule_id": ScheduleId::new().to_string() }),
            "a schedule that does not exist",
        ),
    ] {
        let refused = call(&fixture, erin, "meta/schedule/pause", params).await;
        assert_eq!(
            error_of(&refused),
            (json!(-32001), json!("schedule_not_found")),
            "{why}: {refused:#?}"
        );
    }
    let malformed = call(
        &fixture,
        erin,
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": "not-a-schedule-id" }),
    )
    .await;
    assert_eq!(malformed["error"]["code"], json!(-32602), "{malformed:#?}");

    // Neither is listed under an agent, nor touched.
    let listed = call(
        &fixture,
        erin,
        "meta/schedule/list",
        json!({ "identity": "beta" }),
    )
    .await;
    assert_eq!(listed["result"]["schedules"], json!([]), "{listed:#?}");
    assert_eq!(
        fixture.service.get(&authored.schedule_id).await?.phase,
        meerkat::SchedulePhase::Active
    );
    fixture.stop().await;
    Ok(())
}

/// An agent that admits only governed input takes no console schedule: a
/// governed send needs its Console human present, and a fire has none.
#[tokio::test]
async fn a_governed_agent_takes_no_console_schedule() -> TestResult {
    let fixture = fixture(
        "console-schedules-governed",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let governance = ConsoleSendGovernance {
        human: Some(AuthenticatedConsoleHuman::for_test(
            ISSUER,
            &subject_of(ERIN),
        )),
        scope: Caller::human(ERIN).scope(),
        runtime_governed: true,
        ..ConsoleSendGovernance::default()
    };
    let refused = Box::pin(handle_console_runtime_rpc_with_visibility(
        &fixture.runtime,
        None,
        None,
        None,
        None,
        Some(fixture.aggregator.clone()),
        None,
        None,
        None,
        &AllowAllConsoleVisibilityPolicy,
        request("meta/schedule/create", create_params("alpha")),
        true,
        false,
        Some(ERIN),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        &governance,
        &crate::topology_control::TopologyApplyScope::Anonymous,
    ))
    .await;
    assert_eq!(
        error_of(&refused),
        (
            json!(crate::rpc::CAPABILITY_UNAVAILABLE_CODE),
            json!("governed_target")
        ),
        "{refused:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// The Console send path of `fixture` for console schedule fires, under
/// `decisions` and `access`, checking sessions against `session_fences`,
/// with the governed ingress `ingress` installs.
fn doors(
    fixture: &Fixture,
    decisions: crate::runtime::RuntimeDecisionState,
    access: Option<AccessController>,
    session_fences: crate::auth::session_fence::ConsoleSessionFences,
    ingress: crate::console_governed_ingress::ConsoleGovernedIngressSlot,
) -> ConsoleScheduleDoorSlot {
    let doors = ConsoleScheduleDoorSlot::default();
    doors.install(Arc::new(schedules::ConsoleSendDoor {
        decisions,
        runtime: fixture.runtime.clone(),
        aggregator: fixture.aggregator.clone(),
        access,
        identity_runtime: fixture.identity_runtime.clone(),
        console_events: fixture.events.clone(),
        governed_ingress: ingress,
        session_fences,
    }));
    doors
}

/// The Console's decisions at a fire: sign-in required as
/// `require_app_auth` says, writable or not, trusting [`ISSUER`] under the
/// generic OIDC provider and admitting `allowlist` where sign-in is required.
fn console_decisions(
    require_app_auth: bool,
    read_only: bool,
    allowlist: &[&str],
) -> crate::runtime::RuntimeDecisionState {
    let mut decisions = crate::runtime::RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth,
            read_only,
            ..crate::ConsolePolicy::default()
        },
        None,
    );
    decisions.trusted_oidc.discovery_json = trusting(ISSUER);
    decisions.auth = crate::AuthPolicy {
        default_provider: crate::AuthProvider::GenericOidc,
        email_allowlist: strings(allowlist),
    };
    decisions
}

/// An OIDC discovery document naming `issuer`.
fn trusting(issuer: &str) -> String {
    json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks") }).to_string()
}

fn open_decisions(read_only: bool) -> crate::runtime::RuntimeDecisionState {
    console_decisions(false, read_only, &[])
}

/// No host governs Console sends.
fn ungoverned() -> crate::console_governed_ingress::ConsoleGovernedIngressSlot {
    crate::console_governed_ingress::ConsoleGovernedIngressSlot::default()
}

/// A host that governs every Console send and binds none of them.
struct GoverningHost;

impl crate::console_governed_ingress::ConsoleGovernedIngress for GoverningHost {
    fn governs(&self, _identity: &crate::identity_first::AgentIdentity) -> bool {
        true
    }

    fn binder_for(
        &self,
        _identity: &crate::identity_first::AgentIdentity,
        _human: &AuthenticatedConsoleHuman,
    ) -> Result<Arc<dyn crate::identity_first::GovernedInputBinder>, String> {
        Err("this host binds no test send".to_string())
    }
}

/// A governed ingress slot with [`GoverningHost`] installed.
fn governed() -> crate::console_governed_ingress::ConsoleGovernedIngressSlot {
    let slot = crate::console_governed_ingress::ConsoleGovernedIngressSlot::default();
    slot.install(Arc::new(GoverningHost));
    slot
}

/// The id of a console schedule `caller` creates for `alpha`.
async fn create_as(
    fixture: &Fixture,
    caller: Caller<'_>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let created = call(
        fixture,
        caller,
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    if !created["error"].is_null() {
        return Err(format!("{created:#?}").into());
    }
    Ok(created["result"]["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string())
}

/// A fire the Console refused, for a reason that says `why`.
fn refused_because(fired: Result<(), meerkat::HostRunnableError>, why: &str) {
    match fired {
        Err(meerkat::HostRunnableError::Failed { detail }) => {
            assert!(detail.contains(why), "{detail}");
        }
        other => panic!("the Console refuses the fire ({why}): {other:?}"),
    }
}

/// One fire of the stored console schedule `schedule_id` under `key`, as
/// the schedule host hands it to the console schedules' runnable.
async fn fire(
    fixture: &Fixture,
    doors: &ConsoleScheduleDoorSlot,
    schedule_id: &str,
    key: &str,
) -> Result<(), meerkat::HostRunnableError> {
    let stored = fixture
        .service
        .get(&ScheduleId::parse(schedule_id).expect("a schedule id"))
        .await
        .expect("the schedule is stored");
    let meerkat::TargetBinding::HostRunnable(binding) = &stored.target else {
        panic!("a console schedule targets the console's runnable");
    };
    let host = with_console_schedule_runnable(None, doors);
    host.run_occurrence(HostRunnableInvocation {
        occurrence_id: meerkat::OccurrenceId::new(),
        schedule_id: stored.schedule_id.clone(),
        delivery_idempotency_key: key.to_string(),
        runnable: HostRunnableName::parse(CONSOLE_SCHEDULE_RUNNABLE).expect("a runnable name"),
        trigger_time: chrono::Utc::now(),
        params: binding
            .params
            .clone()
            .map(meerkat::HostRunnableParams::into_raw),
    })
    .await
    .map(|_completed| ())
}

/// The console's `user_input` frames of `identity` whose send carried `key`.
async fn sends_under(fixture: &Fixture, identity: &str, key: &str) -> Vec<ConsoleFrame> {
    fixture
        .aggregator
        .query_timeline(ConsoleTimelineQuery {
            identity: Some(identity.to_string()),
            ..ConsoleTimelineQuery::default()
        })
        .await
        .expect("the timeline reads")
        .frames
        .into_iter()
        .filter(|frame| {
            frame.kind == "user_input" && frame.payload["idempotency_key"] == json!(key)
        })
        .collect()
}

/// Wait until the console's send under `key` to `identity` is delivered.
async fn delivered(fixture: &Fixture, identity: &str, key: &str) -> bool {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if sends_under(fixture, identity, key)
                .await
                .iter()
                .any(|frame| frame.status == ConsoleFrameStatus::Delivered)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

/// A fire is a console send by the schedule's sender through the Console's
/// own send path: deduplicated in the sender's send scope under the
/// occurrence's delivery key (a redelivered occurrence sends once), labelled
/// a scheduled turn, admitted to the agent, and refused, with nothing sent,
/// once the sender may no longer send to the agent, on a read-only console,
/// or once a signed-in sender's Console sessions were revoked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fire_sends_as_its_sender_through_the_console_send_gate() -> TestResult {
    let fixture = delivering_fixture("console-schedules-fire").await?;
    let controller = enforced(vec![grant(
        "erin-drives-alpha",
        ERIN,
        &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
        "alpha",
    )])?;
    let created = call(
        &fixture,
        Caller::human(ERIN).under(&controller),
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let writable = doors(
        &fixture,
        open_decisions(false),
        Some(controller.clone()),
        fences.clone(),
        ungoverned(),
    );

    let key = format!("schedule:{schedule_id}:occurrence:1");
    fire(&fixture, &writable, &schedule_id, &key).await?;
    let sent = sends_under(&fixture, "alpha", &key).await;
    assert_eq!(sent.len(), 1, "{sent:#?}");
    let frame = &sent[0];
    assert_eq!(frame.payload["content"], json!("Post the morning summary."));
    assert_eq!(frame.payload["origin"], json!("console:schedule"));
    assert_eq!(frame.payload["origin_kind"], json!("scheduler"));
    assert_eq!(
        frame.dedupe_key,
        Caller::human(ERIN).scope().dedupe_key(
            &frame.runtime_key,
            "alpha",
            "console:schedule",
            &key
        ),
        "the send is deduplicated in its sender's own send scope"
    );
    assert!(
        delivered(&fixture, "alpha", &key).await,
        "the agent admitted the scheduled send: {:#?}",
        sends_under(&fixture, "alpha", &key).await
    );
    // A redelivered occurrence is the same send.
    fire(&fixture, &writable, &schedule_id, &key).await?;
    assert_eq!(sends_under(&fixture, "alpha", &key).await.len(), 1);

    // Judged at the fire, under the live policy: once an edit of the
    // Console's access policy leaves the sender only `agent.view`, the same
    // send path sends nothing.
    controller.upsert_rule(grant(
        "erin-drives-alpha",
        ERIN,
        &[ACTION_AGENT_VIEW],
        "alpha",
    ))?;
    let refused_key = format!("schedule:{schedule_id}:occurrence:2");
    refused_because(
        fire(&fixture, &writable, &schedule_id, &refused_key).await,
        "not allowed to send",
    );
    assert!(
        sends_under(&fixture, "alpha", &refused_key)
            .await
            .is_empty()
    );

    // A read-only console sends nothing either.
    let read_only = doors(
        &fixture,
        open_decisions(true),
        Some(controller),
        fences.clone(),
        ungoverned(),
    );
    let refused = fire(&fixture, &read_only, &schedule_id, &refused_key).await;
    assert!(
        matches!(&refused, Err(meerkat::HostRunnableError::Failed { detail }) if detail.contains("read-only")),
        "{refused:?}"
    );

    // Revoking a signed-in sender's sessions ends what those sessions
    // defined.
    fences.revoke_sessions(ERIN, std::time::SystemTime::now() + Duration::from_secs(5))?;
    let refused = fire(&fixture, &writable, &schedule_id, &refused_key).await;
    assert!(
        matches!(&refused, Err(meerkat::HostRunnableError::Failed { detail }) if detail.contains("revoked")),
        "{refused:?}"
    );
    assert!(
        sends_under(&fixture, "alpha", &refused_key)
            .await
            .is_empty()
    );
    fixture.stop().await;
    Ok(())
}

/// The requester of the agent's turn that committed the user row `content`
/// of `identity`, once the turn committed it.
async fn turn_requester(
    fixture: &Fixture,
    identity: &str,
    content: &str,
) -> Option<meerkat_core::TurnRequester> {
    let member = fixture
        .runtime
        .handle()
        .get_member(&meerkat_mob::ids::AgentIdentity::from(identity))
        .await
        .expect("roster read")
        .expect("the agent is listed");
    let session = member
        .bridge_session_id()
        .expect("the agent has a session")
        .to_string();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let page = fixture
                .runtime
                .read_session_history(&session, 0, None)
                .await
                .expect("history read");
            if let Some(requester) = page.messages.iter().find_map(|message| match message {
                meerkat_core::Message::User(user) if user.text_content() == content => {
                    // TR-S1b, kept as a regression (R2): a schedule's fire
                    // names the schedule's person as requester but is no
                    // person's typed input. Its conversational row carries no
                    // typed-text stamp and no attestation origin.
                    assert!(
                        user.transcript_role.is_conversational(),
                        "TR-S1b: the fired row is conversational"
                    );
                    assert_eq!(
                        user.identity.turn_input, None,
                        "TR-S1b: a scheduled fire carries no typed-text stamp"
                    );
                    assert_eq!(
                        user.identity
                            .requester
                            .as_ref()
                            .and_then(meerkat_core::TurnRequester::origin),
                        None,
                        "TR-S1b: a scheduled fire names no attestation origin"
                    );
                    Some(user.identity.requester.clone())
                }
                _ => None,
            }) {
                return requester;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the agent committed the scheduled send")
}

/// Whether the row right before the agent's committed user row `content` of
/// `identity` is an injected-context row (a sender label), once the agent
/// committed that row.
async fn labelled_before(fixture: &Fixture, identity: &str, content: &str) -> bool {
    let member = fixture
        .runtime
        .handle()
        .get_member(&meerkat_mob::ids::AgentIdentity::from(identity))
        .await
        .expect("roster read")
        .expect("the agent is listed");
    let session = member
        .bridge_session_id()
        .expect("the agent has a session")
        .to_string();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let messages = fixture
                .runtime
                .read_session_history(&session, 0, None)
                .await
                .expect("history read")
                .messages;
            if let Some(index) = messages.iter().position(|message| {
                matches!(message, meerkat_core::Message::User(user)
                    if user.transcript_role == meerkat_core::types::TranscriptUserRole::Conversational
                        && user.text_content() == content)
            }) {
                return index.checked_sub(1).is_some_and(|previous| {
                    matches!(&messages[previous], meerkat_core::Message::User(user)
                        if user.transcript_role
                            == meerkat_core::types::TranscriptUserRole::InjectedContext)
                });
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the agent committed the scheduled send")
}

/// A fire's input carries the schedule's sender as its requester on a
/// console that requires sign-in: the person who last defined the schedule
/// is the direct requester of the agent's turn, as of a live send of
/// theirs, and the fire's frame names that sender.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduled_fire_requester_is_schedule_sender() -> TestResult {
    let fixture = delivering_fixture("console-schedules-requester").await?;
    let controller = enforced(vec![grant(
        "erin-drives-alpha",
        ERIN,
        &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
        "alpha",
    )])?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN).under(&controller)).await?;
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let signed_in = doors(
        &fixture,
        console_decisions(true, false, &[ERIN]),
        Some(controller),
        fences,
        ungoverned(),
    );

    let key = format!("schedule:{schedule_id}:occurrence:1");
    fire(&fixture, &signed_in, &schedule_id, &key).await?;
    let sent = sends_under(&fixture, "alpha", &key).await;
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert_eq!(
        sent[0].payload["sender"]["kind"],
        json!("person"),
        "{sent:#?}"
    );
    assert_eq!(
        sent[0].payload["sender"]["display_name"],
        json!(ERIN),
        "{sent:#?}"
    );
    assert!(
        delivered(&fixture, "alpha", &key).await,
        "the agent admitted the scheduled send"
    );
    let erin = meerkat_core::PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::Human,
        subject_of(ERIN),
        meerkat_core::TrustDomainId::new(format!("oidc-issuer:{ISSUER}"))?,
    )?;
    assert_eq!(
        turn_requester(&fixture, "alpha", "Post the morning summary.").await,
        Some(meerkat_core::TurnRequester::direct(erin)),
        "the fired turn's requester is the schedule's sender"
    );
    // A schedule's fire is no human message: its turn carries no sender
    // label, although the console labels a person's own sends.
    assert!(
        !labelled_before(&fixture, "alpha", "Post the morning summary.").await,
        "a schedule's fire carries no human sender label"
    );
    fixture.stop().await;
    Ok(())
}

/// A fire names its sender as that caller's live sends are named at fire
/// time. A person defined the schedule, and their issuer's tokens are then
/// classified as shared accounts (`shared_account_tokens`): a live token of
/// that issuer names an account many people may hold, not one person. So
/// the fire names no person: its sender is a token principal and its turn
/// has no requester.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fire_after_its_issuer_turned_shared_names_no_person() -> TestResult {
    let fixture = delivering_fixture("console-schedules-shared-issuer").await?;
    let controller = enforced(vec![grant(
        "erin-drives-alpha",
        ERIN,
        &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
        "alpha",
    )])?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN).under(&controller)).await?;
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let mut shared_issuer = console_decisions(true, false, &[ERIN]);
    shared_issuer.trusted_oidc.shared_account_tokens = true;
    let signed_in = doors(
        &fixture,
        shared_issuer,
        Some(controller),
        fences,
        ungoverned(),
    );

    let key = format!("schedule:{schedule_id}:occurrence:1");
    fire(&fixture, &signed_in, &schedule_id, &key).await?;
    let sent = sends_under(&fixture, "alpha", &key).await;
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert_eq!(
        sent[0].payload["sender"]["kind"],
        json!("token_principal"),
        "a shared account's fire names no person: {sent:#?}"
    );
    assert!(
        delivered(&fixture, "alpha", &key).await,
        "the agent admitted the scheduled send"
    );
    assert_eq!(
        turn_requester(&fixture, "alpha", "Post the morning summary.").await,
        None,
        "the fired turn has no requester"
    );
    fixture.stop().await;
    Ok(())
}

/// On a console without sign-in, a fire sends as the console's shared
/// account, deduplicated in that account's scope, as its live sends are.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fire_on_a_console_without_sign_in_sends_as_its_shared_account() -> TestResult {
    let fixture = delivering_fixture("console-schedules-open-fire").await?;
    let created = call(
        &fixture,
        Caller::open_console(),
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();
    let open = doors(
        &fixture,
        open_decisions(false),
        None,
        crate::auth::session_fence::ConsoleSessionFences::detached(),
        ungoverned(),
    );
    let key = format!("schedule:{schedule_id}:occurrence:1");
    fire(&fixture, &open, &schedule_id, &key).await?;
    let sent = sends_under(&fixture, "alpha", &key).await;
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert_eq!(
        sent[0].dedupe_key,
        ConsoleSendScope::Anonymous.dedupe_key(
            &sent[0].runtime_key,
            "alpha",
            "console:schedule",
            &key
        ),
        "the send is deduplicated in the open Console's shared account"
    );
    assert!(
        delivered(&fixture, "alpha", &key).await,
        "the agent admitted the scheduled send: {:#?}",
        sends_under(&fixture, "alpha", &key).await
    );
    fixture.stop().await;
    Ok(())
}

/// On an identity-first Console (the production lane) a fire is the
/// identity-first console send by its sender: delivered to the durable
/// identity once under its key, and refused, with nothing sent, once a host
/// governs the agent's input, since a fire has no Console human present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fire_takes_the_identity_first_send_and_its_governance() -> TestResult {
    let fixture = identity_first_fixture("console-schedules-identity-first").await?;
    let identity_runtime = fixture
        .identity_runtime
        .clone()
        .ok_or("an identity-first fixture")?;
    let alpha = crate::identity_first::AgentIdentity::parse("alpha")
        .map_err(|error| format!("{error:?}"))?;
    assert!(
        identity_runtime.status(&alpha).await.is_ok(),
        "the identity runtime holds the agent, so its sends take the identity-first lane"
    );
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let fences = crate::auth::session_fence::ConsoleSessionFences::detached();

    let open = doors(
        &fixture,
        open_decisions(false),
        None,
        fences.clone(),
        ungoverned(),
    );
    let key = format!("schedule:{schedule_id}:occurrence:1");
    fire(&fixture, &open, &schedule_id, &key).await?;
    let sent = sends_under(&fixture, "alpha", &key).await;
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert_eq!(sent[0].payload["origin"], json!("console:schedule"));
    assert_eq!(sent[0].payload["origin_kind"], json!("scheduler"));
    assert!(
        delivered(&fixture, "alpha", &key).await,
        "the identity admitted the scheduled send: {:#?}",
        sends_under(&fixture, "alpha", &key).await
    );
    fire(&fixture, &open, &schedule_id, &key).await?;
    assert_eq!(
        sends_under(&fixture, "alpha", &key).await.len(),
        1,
        "a redelivered occurrence is the same send"
    );

    let governing = doors(&fixture, open_decisions(false), None, fences, governed());
    let governed_key = format!("schedule:{schedule_id}:occurrence:2");
    let refused = fire(&fixture, &governing, &schedule_id, &governed_key).await;
    assert!(
        matches!(refused, Err(meerkat::HostRunnableError::Failed { .. })),
        "a governed agent refuses a fire: {refused:?}"
    );
    assert!(
        sends_under(&fixture, "alpha", &governed_key)
            .await
            .is_empty()
    );
    fixture.stop().await;
    Ok(())
}

/// An agent whose input became governed after its schedule was defined
/// takes none of its fires, on the member lane too: nothing is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fire_to_an_agent_governed_since_is_refused() -> TestResult {
    let fixture = delivering_fixture("console-schedules-governed-since").await?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let governing = doors(
        &fixture,
        open_decisions(false),
        None,
        crate::auth::session_fence::ConsoleSessionFences::detached(),
        governed(),
    );
    let key = format!("schedule:{schedule_id}:occurrence:1");
    let refused = fire(&fixture, &governing, &schedule_id, &key).await;
    assert!(
        matches!(refused, Err(meerkat::HostRunnableError::Failed { .. })),
        "a governed agent refuses a fire: {refused:?}"
    );
    assert!(sends_under(&fixture, "alpha", &key).await.is_empty());
    fixture.stop().await;
    Ok(())
}

/// A fire is admitted as a live request by its sender is, under the
/// Console's decisions at the fire: an anonymous schedule stops once the
/// Console requires sign-in, and a sender removed from the allowlist, or
/// signed in under an issuer the Console no longer trusts, sends nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fires_follow_the_consoles_admission_of_their_sender() -> TestResult {
    let fixture = delivering_fixture("console-schedules-admission").await?;
    let anonymous = create_as(&fixture, Caller::open_console()).await?;
    let erins = create_as(&fixture, Caller::human(ERIN)).await?;
    let fences = crate::auth::session_fence::ConsoleSessionFences::detached();
    let door = |decisions| doors(&fixture, decisions, None, fences.clone(), ungoverned());
    let open = door(console_decisions(false, false, &[]));
    let signed_in = door(console_decisions(true, false, &[ERIN]));
    let erin_removed = door(console_decisions(true, false, &[]));
    let mut moved = console_decisions(true, false, &[ERIN]);
    moved.trusted_oidc.discovery_json = trusting("https://other.example.test");
    let other_issuer = door(moved);

    fire(&fixture, &open, &anonymous, "anonymous-1").await?;
    fire(&fixture, &signed_in, &erins, "erin-1").await?;
    for key in ["anonymous-1", "erin-1"] {
        assert_eq!(sends_under(&fixture, "alpha", key).await.len(), 1, "{key}");
    }

    refused_because(
        fire(&fixture, &signed_in, &anonymous, "anonymous-2").await,
        "requires sign-in",
    );
    refused_because(
        fire(&fixture, &erin_removed, &erins, "erin-2").await,
        "no longer admits",
    );
    refused_because(
        fire(&fixture, &other_issuer, &erins, "erin-3").await,
        "no longer trusts",
    );
    for key in ["anonymous-2", "erin-2", "erin-3"] {
        assert!(
            sends_under(&fixture, "alpha", key).await.is_empty(),
            "{key}"
        );
    }
    fixture.stop().await;
    Ok(())
}

/// A service identity's schedule sends as it, in its own send scope, and
/// stops with its revoked Console sessions, as a human's does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_identitys_schedule_stops_with_its_revoked_sessions() -> TestResult {
    let fixture = delivering_fixture("console-schedules-service").await?;
    let schedule_id = create_as(&fixture, Caller::service(ROBOT)).await?;
    assert_eq!(
        stored_send(&fixture, &schedule_id).await?.sender,
        ConsoleScheduleSender::Principal {
            principal: ROBOT.to_string(),
            provider: "service_identity".to_string(),
            issuer: ISSUER.to_string(),
        }
    );
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let signed_in = doors(
        &fixture,
        console_decisions(true, false, &[ROBOT]),
        None,
        fences.clone(),
        ungoverned(),
    );
    let key = format!("schedule:{schedule_id}:occurrence:1");
    fire(&fixture, &signed_in, &schedule_id, &key).await?;
    let sent = sends_under(&fixture, "alpha", &key).await;
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert_eq!(
        sent[0].dedupe_key,
        Caller::service(ROBOT).scope().dedupe_key(
            &sent[0].runtime_key,
            "alpha",
            "console:schedule",
            &key
        ),
        "the send is deduplicated in the service identity's own send scope"
    );

    fences.revoke_sessions(ROBOT, std::time::SystemTime::now() + Duration::from_secs(5))?;
    let revoked_key = format!("schedule:{schedule_id}:occurrence:2");
    refused_because(
        fire(&fixture, &signed_in, &schedule_id, &revoked_key).await,
        "revoked",
    );
    assert!(
        sends_under(&fixture, "alpha", &revoked_key)
            .await
            .is_empty()
    );
    fixture.stop().await;
    Ok(())
}

/// An update and a resume write at the revision they read: a write that
/// raced a change made meanwhile is answered as a typed conflict and
/// changes nothing, so it never reverts the other change.
#[tokio::test]
async fn a_write_at_a_stale_revision_is_a_conflict() -> TestResult {
    let fixture = fixture(
        "console-schedules-conflict",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let id = ScheduleId::parse(&schedule_id)?;
    let read = fixture.service.get(&id).await?;
    let changed = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/update",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "content": "Changed meanwhile.",
            "take_over_sender": true,
        }),
    )
    .await;
    assert!(changed["error"].is_null(), "{changed:#?}");
    let now = fixture.service.get(&id).await?;
    assert_ne!(now.revision, read.revision);

    let stale = schedules::update_at(
        &fixture.service,
        &id,
        read.revision,
        meerkat::UpdateScheduleRequest {
            name: Some("Stale".to_string()),
            ..meerkat::UpdateScheduleRequest::default()
        },
    )
    .await;
    let conflict = match stale {
        Err(conflict) => conflict,
        Ok(written) => panic!("a stale write is refused, not written: {written:?}"),
    };
    assert_eq!(conflict.code, -32009);
    assert_eq!(
        conflict.data,
        Some(json!({
            "kind": "schedule_revision_conflict",
            "expected_revision": read.revision.0,
            "actual_revision": now.revision.0,
        }))
    );
    let after = fixture.service.get(&id).await?;
    assert_eq!(
        after.revision, now.revision,
        "the stale write changed nothing"
    );
    assert_eq!(after.config.name.as_deref(), Some("Morning summary"));
    assert_eq!(
        stored_send(&fixture, &schedule_id).await?.content,
        "Changed meanwhile."
    );
    fixture.stop().await;
    Ok(())
}

/// The Console router over `fixture`, signing callers in with the console
/// test issuer's tokens and checking their sessions against `fences`.
fn signed_in_router(
    fixture: &Fixture,
    fences: crate::auth::session_fence::ConsoleSessionFences,
) -> axum::Router {
    let mut state = super::console_human_admission_tests::state();
    state.runtime = Some(fixture.runtime.clone());
    state.console_aggregator = Some(fixture.aggregator.clone());
    state.console_events = Some(fixture.events.clone());
    state.session_fences = fences;
    console_json_router_with_state(state)
}

/// `body` posted to `path` on `router` with `token` as its bearer: the
/// status and the JSON answer.
async fn post_as(
    router: &axum::Router,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(axum::body::Body::from(body.to_string()))
                .expect("console request"),
        )
        .await
        .expect("console response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("console body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// A Console change may carry the caller scope its client believes it acts
/// as (`expected_caller_scope`, the `caller_scope` of `meta/console/whoami`).
/// A change made for another caller is refused as `caller_changed`, and
/// nothing is sent or written; the caller's own scope passes and is no
/// parameter of the change itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutation_with_other_expected_scope_is_refused() -> TestResult {
    use super::console_token_lapse_tests::{console_rpc_as, human_claims, now_seconds, token};
    let fixture = delivering_fixture("console-caller-changed").await?;
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let router = signed_in_router(&fixture, fences);
    let expiry = now_seconds() + 3600;
    let bob = token(human_claims(), Some(expiry));
    let robot = token(json!({ "email": "svc:robot" }), Some(expiry));
    let scope_of = |answer: &Value| {
        answer["result"]["caller_scope"]
            .as_str()
            .map(str::to_string)
    };
    let own =
        scope_of(&console_rpc_as(&router, Some(&bob), "meta/console/whoami", json!({})).await)
            .ok_or("the caller's scope")?;
    let other =
        scope_of(&console_rpc_as(&router, Some(&robot), "meta/console/whoami", json!({})).await)
            .ok_or("another caller's scope")?;
    assert_ne!(own, other);
    let send = |key: &str, scope: &str| {
        json!({
            "identity": "alpha",
            "content": "Status, please.",
            "origin": "console",
            "idempotency_key": key,
            "expected_caller_scope": scope,
        })
    };

    let refused = console_rpc_as(
        &router,
        Some(&bob),
        "meta/console/send",
        send("changed-rpc-send", &other),
    )
    .await;
    assert_eq!(
        refused["error"]["data"]["kind"],
        json!("caller_changed"),
        "{refused:#?}"
    );
    assert!(
        sends_under(&fixture, "alpha", "changed-rpc-send")
            .await
            .is_empty(),
        "nothing was sent"
    );
    let sent = console_rpc_as(
        &router,
        Some(&bob),
        "meta/console/send",
        send("own-rpc-send", &own),
    )
    .await;
    assert!(sent["error"].is_null(), "{sent:#?}");

    let (status, refused) = post_as(
        &router,
        "/console/send",
        &bob,
        send("changed-rest-send", &other),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused:#?}");
    assert_eq!(refused["error"], json!("caller_changed"), "{refused:#?}");
    assert!(
        sends_under(&fixture, "alpha", "changed-rest-send")
            .await
            .is_empty(),
        "nothing was sent"
    );
    let (status, sent) = post_as(&router, "/console/send", &bob, send("own-rest-send", &own)).await;
    assert_eq!(status, StatusCode::OK, "{sent:#?}");

    let mut params = create_params("alpha");
    params["expected_caller_scope"] = json!(own);
    let created = console_rpc_as(&router, Some(&bob), "meta/schedule/create", params).await;
    assert!(created["error"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();
    let refused = console_rpc_as(
        &router,
        Some(&bob),
        "meta/schedule/update",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "content": "Changed for another caller.",
            "expected_caller_scope": other,
        }),
    )
    .await;
    assert_eq!(
        refused["error"]["data"]["kind"],
        json!("caller_changed"),
        "{refused:#?}"
    );
    assert_eq!(
        stored_send(&fixture, &schedule_id).await?.content,
        "Post the morning summary.",
        "the schedule is as it was"
    );
    fixture.stop().await;
    Ok(())
}

/// A schedule change that waited before it writes checks again that its
/// caller's login is current: once the caller's Console sessions were
/// revoked meanwhile, the change is refused as `caller_not_current` and the
/// schedule stays as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutation_rechecks_caller_after_await() -> TestResult {
    use super::console_human_admission_tests::BOB;
    use super::console_token_lapse_tests::{console_rpc_as, human_claims, now_seconds, token};
    let fixture = delivering_fixture("console-caller-not-current").await?;
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let router = signed_in_router(&fixture, fences.clone());
    let bob = token(human_claims(), Some(now_seconds() + 3600));
    let created = console_rpc_as(
        &router,
        Some(&bob),
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();

    let hold = crate::console_schedules::ScheduleWriteHold::default();
    fixture
        .runtime
        .console_schedules()
        .attached()
        .ok_or("the schedules are attached")?
        .hold_writes(hold.clone());
    let update = tokio::spawn({
        let router = router.clone();
        let bob = bob.clone();
        let schedule_id = schedule_id.clone();
        async move {
            console_rpc_as(
                &router,
                Some(&bob),
                "meta/schedule/update",
                json!({
                    "identity": "alpha",
                    "schedule_id": schedule_id,
                    "content": "Changed after the login ended.",
                }),
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), hold.reached())
        .await
        .map_err(|_| "the update reached the hold")?;
    fences.revoke_sessions(BOB, std::time::SystemTime::now() + Duration::from_secs(5))?;
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), update).await??;
    assert_eq!(
        answer["error"]["data"]["kind"],
        json!("caller_not_current"),
        "{answer:#?}"
    );
    assert_eq!(
        stored_send(&fixture, &schedule_id).await?.content,
        "Post the morning summary.",
        "nothing was written"
    );
    fixture.stop().await;
    Ok(())
}

/// Signing out of one console login ends that login only: the schedule its
/// caller defined keeps firing as that caller, since a schedule holds the
/// mandate it was defined with, not a login. A sign-out that folds the
/// person's oldest sign-outs into a fence of every login of the person (the
/// 65th kept sign-out) stops no schedule either: the fold revokes nothing.
/// Revoking the caller's principal still stops it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sign_out_keeps_schedules() -> TestResult {
    use super::console_human_admission_tests::{BOB, state};
    use super::console_sign_out_tests::{accepted, console_answer, login_token};
    use super::console_token_lapse_tests::now_seconds;
    let fixture = delivering_fixture("console-schedules-sign-out").await?;
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let router = signed_in_router(&fixture, fences.clone());
    let login = login_token(Some("schedule-login"), now_seconds() - 60);
    let (_, created) = console_answer(
        &router,
        Some(&login),
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();

    let (_, signed_out) =
        console_answer(&router, Some(&login), "meta/console/sign_out", json!({})).await;
    assert!(signed_out["error"].is_null(), "{signed_out:#?}");
    assert!(
        !accepted(&router, &login).await,
        "the login that signed out is refused"
    );
    // 64 more sign-outs, each fenced at a second of its own after this one
    // (their tokens say they were issued then): the last folds the oldest,
    // this login's, into a fence of every login of the person.
    let later = now_seconds() + 1_000;
    for other in 0..64_u64 {
        let token = login_token(Some(&format!("later-login-{other}")), later + other);
        let (_, signed_out) =
            console_answer(&router, Some(&token), "meta/console/sign_out", json!({})).await;
        assert!(signed_out["error"].is_null(), "{signed_out:#?}");
    }
    assert!(
        !accepted(
            &router,
            &login_token(Some("another-login"), now_seconds() - 60)
        )
        .await,
        "the fold signed out every older login of the person"
    );

    let doors = doors(
        &fixture,
        state().decisions,
        None,
        fences.clone(),
        ungoverned(),
    );
    let key = format!("schedule:{schedule_id}:occurrence:1");
    fire(&fixture, &doors, &schedule_id, &key).await?;
    assert_eq!(sends_under(&fixture, "alpha", &key).await.len(), 1);
    assert!(
        delivered(&fixture, "alpha", &key).await,
        "the agent admitted the scheduled send: {:#?}",
        sends_under(&fixture, "alpha", &key).await
    );

    fences.revoke_sessions(BOB, std::time::SystemTime::now() + Duration::from_secs(5))?;
    let refused_key = format!("schedule:{schedule_id}:occurrence:2");
    refused_because(
        fire(&fixture, &doors, &schedule_id, &refused_key).await,
        "revoked",
    );
    assert!(
        sends_under(&fixture, "alpha", &refused_key)
            .await
            .is_empty()
    );
    fixture.stop().await;
    Ok(())
}

/// The revision `schedule_id` is at now.
async fn revision_of(
    fixture: &Fixture,
    schedule_id: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(fixture
        .service
        .get(&ScheduleId::parse(schedule_id)?)
        .await?
        .revision
        .0)
}

/// Two edits made from one revision (two tabs, or two people, read the
/// schedule at the same revision): the first is written, and the second,
/// which carries the revision it was made against, is refused as a conflict
/// and changes nothing, so it never overwrites the first. A resume from a
/// revision the schedule moved past is refused alike; one from the current
/// revision resumes.
#[tokio::test]
async fn stale_schedule_edit_refused() -> TestResult {
    let fixture = fixture(
        "console-schedules-stale-edit",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let seen = revision_of(&fixture, &schedule_id).await?;
    let edit = |content: &str| {
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "content": content,
            "expected_revision": seen,
        })
    };
    let first = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/update",
        edit("The first edit."),
    )
    .await;
    assert!(first["error"].is_null(), "{first:#?}");
    let second = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/update",
        edit("The second edit."),
    )
    .await;
    assert_eq!(
        error_of(&second),
        (json!(-32009), json!("schedule_revision_conflict")),
        "{second:#?}"
    );
    assert_eq!(second["error"]["data"]["expected_revision"], json!(seen));
    assert_eq!(
        second["error"]["data"]["actual_revision"],
        first["result"]["revision"]
    );
    assert_eq!(
        stored_send(&fixture, &schedule_id).await?.content,
        "The first edit.",
        "the first edit stands"
    );

    let paused = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    let stale = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/resume",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "expected_revision": seen,
        }),
    )
    .await;
    assert_eq!(
        error_of(&stale),
        (json!(-32009), json!("schedule_revision_conflict")),
        "{stale:#?}"
    );
    assert_eq!(
        fixture
            .service
            .get(&ScheduleId::parse(&schedule_id)?)
            .await?
            .phase,
        meerkat::SchedulePhase::Paused,
        "the stale resume changed nothing"
    );
    let resumed = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/resume",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "expected_revision": paused["result"]["revision"],
        }),
    )
    .await;
    assert_eq!(resumed["result"]["phase"], json!("active"), "{resumed:#?}");

    let malformed = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/update",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "content": "Changed.",
            "expected_revision": "latest",
        }),
    )
    .await;
    assert_eq!(malformed["error"]["code"], json!(-32602), "{malformed:#?}");
    fixture.stop().await;
    Ok(())
}

/// Changing who a schedule sends as is an explicit takeover. Another
/// caller's update or resume without `take_over_sender` is refused, names
/// who the schedule sends as, and changes nothing; with it, the caller
/// becomes the sender and the schedule's sender history names the sender it
/// replaced. The sender's own edits need no takeover and keep the history.
#[tokio::test]
async fn sender_takeover_is_explicit() -> TestResult {
    let fixture = fixture(
        "console-schedules-takeover",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let update = |content: &str| {
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "content": content,
        })
    };
    let take_over = |mut params: Value| {
        params["take_over_sender"] = json!(true);
        params
    };

    let refused = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/update",
        update("Changed by another caller."),
    )
    .await;
    assert_eq!(
        error_of(&refused),
        (json!(-32009), json!("sender_takeover_required")),
        "{refused:#?}"
    );
    assert_eq!(
        refused["error"]["data"]["sender"],
        json!(ERIN),
        "the refusal names who the schedule sends as"
    );
    let kept = stored_send(&fixture, &schedule_id).await?;
    assert_eq!(kept.content, "Post the morning summary.");
    assert_eq!(kept.sender.principal(), Some(ERIN));

    let own = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/update",
        update("Changed by its sender."),
    )
    .await;
    assert!(own["error"].is_null(), "{own:#?}");
    assert_eq!(own["result"]["sender"], json!(ERIN));
    assert_eq!(own["result"]["sender_history"], json!([]), "{own:#?}");

    let taken = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/update",
        take_over(update("Taken over.")),
    )
    .await;
    assert!(taken["error"].is_null(), "{taken:#?}");
    assert_eq!(taken["result"]["sender"], json!(FRANK));
    let history = taken["result"]["sender_history"]
        .as_array()
        .ok_or("the schedule answers its sender history")?;
    assert_eq!(history.len(), 1, "{taken:#?}");
    assert_eq!(history[0]["sender"], json!(ERIN));
    assert!(history[0]["defined_at_utc"].is_string(), "{taken:#?}");
    assert!(history[0]["replaced_at_utc"].is_string(), "{taken:#?}");
    assert_eq!(
        stored_send(&fixture, &schedule_id)
            .await?
            .sender
            .principal(),
        Some(FRANK)
    );

    let again = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/update",
        update("Changed by its new sender."),
    )
    .await;
    assert!(again["error"].is_null(), "{again:#?}");
    assert_eq!(
        again["result"]["sender_history"], taken["result"]["sender_history"],
        "the sender's own edit keeps the history"
    );

    // A resume by another caller is a takeover too; a pause is not.
    let paused = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    assert_eq!(paused["result"]["sender"], json!(FRANK));
    let resume = json!({ "identity": "alpha", "schedule_id": schedule_id });
    let refused = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/resume",
        resume.clone(),
    )
    .await;
    assert_eq!(
        error_of(&refused),
        (json!(-32009), json!("sender_takeover_required")),
        "{refused:#?}"
    );
    assert_eq!(
        fixture
            .service
            .get(&ScheduleId::parse(&schedule_id)?)
            .await?
            .phase,
        meerkat::SchedulePhase::Paused,
        "the refused resume changed nothing"
    );
    let resumed = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/resume",
        take_over(resume),
    )
    .await;
    assert_eq!(resumed["result"]["phase"], json!("active"), "{resumed:#?}");
    assert_eq!(resumed["result"]["sender"], json!(ERIN));
    let senders = resumed["result"]["sender_history"]
        .as_array()
        .map(|history| {
            history
                .iter()
                .map(|former| former["sender"].clone())
                .collect::<Vec<_>>()
        });
    assert_eq!(
        senders,
        Some(vec![json!(ERIN), json!(FRANK)]),
        "{resumed:#?}"
    );

    let malformed = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/update",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "content": "Changed.",
            "take_over_sender": "yes",
        }),
    )
    .await;
    assert_eq!(malformed["error"]["code"], json!(-32602), "{malformed:#?}");
    fixture.stop().await;
    Ok(())
}

/// `schedule.manage` lets a deployment restrict who changes console
/// schedules: under a configuration that names it, a caller who may send to
/// the agent but holds no `schedule.manage` lists its schedules and changes
/// none. A configuration that names no schedule action keeps the rule it
/// had: `agent.send` on the agent lets a caller change its schedules, and
/// the configuration says so from then on.
#[tokio::test]
async fn schedule_manage_restricts_schedule_writes() -> TestResult {
    let fixture = fixture(
        "console-schedules-manage",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let restricted = enforced(vec![
        grant(
            "frank-schedules-alpha",
            FRANK,
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND, "schedule.manage"],
            "alpha",
        ),
        grant(
            "erin-messages-alpha",
            ERIN,
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
            "alpha",
        ),
    ])
    .map_err(|error| format!("a configuration may name schedule.manage: {error}"))?;
    let frank = Caller::human(FRANK).under(&restricted);
    let erin = Caller::human(ERIN).under(&restricted);
    let schedule_id = create_as(&fixture, frank).await?;
    let denied = |answer: &Value, method: &str| {
        assert_eq!(
            error_of(answer),
            (json!(ACCESS_DENIED_RPC_CODE), json!("access_denied")),
            "{method}: {answer:#?}"
        );
        assert_eq!(
            answer["error"]["data"]["action"],
            json!("schedule.manage"),
            "{method}: {answer:#?}"
        );
    };
    let by_id = json!({ "identity": "alpha", "schedule_id": schedule_id });
    for (method, params) in [
        ("meta/schedule/create", create_params("alpha")),
        (
            "meta/schedule/update",
            json!({
                "identity": "alpha",
                "schedule_id": schedule_id,
                "content": "Changed.",
                "take_over_sender": true,
            }),
        ),
        ("meta/schedule/pause", by_id.clone()),
        ("meta/schedule/resume", by_id.clone()),
        ("meta/schedule/delete", by_id.clone()),
    ] {
        denied(&call(&fixture, erin, method, params).await, method);
    }
    let listed = call(
        &fixture,
        erin,
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    assert_eq!(
        listed["result"]["schedules"].as_array().map(Vec::len),
        Some(1),
        "{listed:#?}"
    );
    assert_eq!(
        stored_send(&fixture, &schedule_id).await?.content,
        "Post the morning summary.",
        "the schedule is as it was"
    );

    // A configuration that names no schedule action: `agent.send` still lets
    // a caller change the agent's schedules.
    let earlier = enforced(vec![grant(
        "erin-messages-alpha",
        ERIN,
        &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
        "alpha",
    )])?;
    let erin = Caller::human(ERIN).under(&earlier);
    let own = create_as(&fixture, erin).await?;
    let paused = call(
        &fixture,
        erin,
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": own }),
    )
    .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    let (config, _) = earlier.snapshot();
    assert!(
        config.rules[0]
            .actions
            .iter()
            .any(|action| action == "schedule.manage"),
        "the configuration grants schedule.manage where it grants agent.send: {config:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A configuration that names no schedule action keeps `agent.send`
/// sufficient for schedule changes when an edit puts it in place, as it does
/// when it is loaded: the edit extends it as the load does.
#[tokio::test]
async fn edited_configuration_without_schedule_actions_keeps_send_sufficient() -> TestResult {
    let fixture = fixture(
        "console-schedules-edited-naive",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let controller = enforced(vec![grant(
        "frank-schedules-alpha",
        FRANK,
        &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND, "schedule.manage"],
        "alpha",
    )])?;
    controller.replace_config(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![grant(
            "erin-messages-alpha",
            ERIN,
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
            "alpha",
        )],
        ..crate::access::AccessControlConfig::default()
    })?;
    let erin = Caller::human(ERIN).under(&controller);
    let created = call(
        &fixture,
        erin,
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert!(
        created["error"].is_null(),
        "agent.send still lets erin change schedules: {created:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A resume that would take the schedule over from its sender, refused for
/// want of `take_over_sender`, writes nothing: the revision, the sender and
/// the history stay as they were.
#[tokio::test]
async fn refused_resume_takeover_writes_nothing() -> TestResult {
    let fixture = fixture(
        "console-schedules-refused-resume",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let paused = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    let before = revision_of(&fixture, &schedule_id).await?;
    let refused = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/resume",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(
        error_of(&refused),
        (json!(-32009), json!("sender_takeover_required")),
        "{refused:#?}"
    );
    assert_eq!(
        revision_of(&fixture, &schedule_id).await?,
        before,
        "the refused resume wrote nothing"
    );
    let kept = stored_send(&fixture, &schedule_id).await?;
    assert_eq!(kept.sender.principal(), Some(ERIN));
    assert!(kept.sender_history.is_empty(), "{kept:#?}");
    fixture.stop().await;
    Ok(())
}

/// A resume answers the schedule as Core resumed it: when another change
/// replaced the sender and the message between the resume's own write and
/// the resume, the answer names that sender and message and the revision
/// the schedule is at, never the resume's earlier view of it.
#[tokio::test]
async fn resume_answers_the_schedule_as_it_resumed() -> TestResult {
    let fixture = fixture(
        "console-schedules-resume-answer",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let paused = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    )
    .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    let seen = revision_of(&fixture, &schedule_id).await?;
    let hold = crate::console_schedules::ScheduleWriteHold::default();
    fixture
        .runtime
        .console_schedules()
        .attached()
        .ok_or("the schedules are attached")?
        .hold_resumes(hold.clone());
    let resume = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/resume",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "expected_revision": seen,
            "take_over_sender": true,
        }),
    );
    let meanwhile = async {
        tokio::time::timeout(Duration::from_secs(30), hold.reached())
            .await
            .map_err(|_| "the resume reached its hold")?;
        let taken_back = call(
            &fixture,
            Caller::human(ERIN),
            "meta/schedule/update",
            json!({
                "identity": "alpha",
                "schedule_id": schedule_id,
                "content": "Changed meanwhile.",
                "take_over_sender": true,
            }),
        )
        .await;
        hold.release();
        Ok::<Value, Box<dyn std::error::Error + Send + Sync>>(taken_back)
    };
    let (resumed, taken_back) = tokio::join!(resume, meanwhile);
    let taken_back = taken_back?;
    assert!(taken_back["error"].is_null(), "{taken_back:#?}");
    assert!(resumed["error"].is_null(), "{resumed:#?}");
    assert_eq!(resumed["result"]["phase"], json!("active"), "{resumed:#?}");
    assert_eq!(
        resumed["result"]["sender"],
        json!(ERIN),
        "the answer names who the schedule sends as: {resumed:#?}"
    );
    assert_eq!(
        resumed["result"]["content"],
        json!("Changed meanwhile."),
        "{resumed:#?}"
    );
    assert_eq!(
        resumed["result"]["revision"],
        json!(revision_of(&fixture, &schedule_id).await?),
        "{resumed:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A pause answers the schedule as Core paused it: when another caller took
/// the schedule over between the pause's read and the pause, the answer
/// names that sender and message and the revision the schedule is at, never
/// the pause's earlier view of it.
#[tokio::test]
async fn pause_answers_the_schedule_as_it_paused() -> TestResult {
    let fixture = fixture(
        "console-schedules-pause-answer",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let hold = crate::console_schedules::ScheduleWriteHold::default();
    fixture
        .runtime
        .console_schedules()
        .attached()
        .ok_or("the schedules are attached")?
        .hold_writes(hold.clone());
    let pause = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
    );
    let meanwhile = async {
        tokio::time::timeout(Duration::from_secs(30), hold.reached())
            .await
            .map_err(|_| "the pause reached its hold")?;
        let taken = call(
            &fixture,
            Caller::human(FRANK),
            "meta/schedule/update",
            json!({
                "identity": "alpha",
                "schedule_id": schedule_id,
                "content": "Changed meanwhile.",
                "take_over_sender": true,
            }),
        )
        .await;
        hold.release();
        Ok::<Value, Box<dyn std::error::Error + Send + Sync>>(taken)
    };
    let (paused, taken) = tokio::join!(pause, meanwhile);
    let taken = taken?;
    assert!(taken["error"].is_null(), "{taken:#?}");
    assert!(paused["error"].is_null(), "{paused:#?}");
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    assert_eq!(
        paused["result"]["sender"],
        json!(FRANK),
        "the answer names who the schedule sends as: {paused:#?}"
    );
    assert_eq!(
        paused["result"]["content"],
        json!("Changed meanwhile."),
        "{paused:#?}"
    );
    assert_eq!(
        paused["result"]["revision"],
        json!(revision_of(&fixture, &schedule_id).await?),
        "{paused:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A pause, a delete and a create take no `expected_revision` and no
/// `take_over_sender`: Core pauses and deletes a schedule as it is, and none
/// of them makes anyone the sender in another's place. Naming either is
/// refused as `unsupported_param` and changes nothing, rather than read as a
/// condition the call would not keep; a null names neither.
#[tokio::test]
async fn pause_delete_and_create_refuse_conditions_they_do_not_keep() -> TestResult {
    let fixture = fixture(
        "console-schedules-unkept",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN)).await?;
    let seen = revision_of(&fixture, &schedule_id).await?;
    let by_id = |param: &str, value: Value| {
        let mut params = json!({ "identity": "alpha", "schedule_id": schedule_id });
        params[param] = value;
        params
    };
    let mut create = create_params("alpha");
    create["expected_revision"] = json!(seen);
    let mut create_takeover = create_params("alpha");
    create_takeover["take_over_sender"] = json!(true);
    for (method, params, param) in [
        (
            "meta/schedule/pause",
            by_id("expected_revision", json!(seen)),
            "expected_revision",
        ),
        (
            "meta/schedule/pause",
            by_id("take_over_sender", json!(true)),
            "take_over_sender",
        ),
        (
            "meta/schedule/delete",
            by_id("expected_revision", json!(seen)),
            "expected_revision",
        ),
        (
            "meta/schedule/delete",
            by_id("take_over_sender", json!(false)),
            "take_over_sender",
        ),
        ("meta/schedule/create", create, "expected_revision"),
        ("meta/schedule/create", create_takeover, "take_over_sender"),
    ] {
        let answer = call(&fixture, Caller::human(ERIN), method, params).await;
        assert_eq!(
            error_of(&answer),
            (json!(-32602), json!("unsupported_param")),
            "{method}: {answer:#?}"
        );
        assert_eq!(
            answer["error"]["data"]["param"],
            json!(param),
            "{method}: {answer:#?}"
        );
    }
    let schedule = fixture
        .service
        .get(&ScheduleId::parse(&schedule_id)?)
        .await?;
    assert_eq!(
        schedule.phase,
        meerkat::SchedulePhase::Active,
        "nothing was paused or deleted"
    );
    assert_eq!(schedule.revision.0, seen, "nothing was written");
    let listed = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    assert_eq!(
        listed["result"]["schedules"].as_array().map(Vec::len),
        Some(1),
        "nothing was created: {listed:#?}"
    );
    let paused = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/pause",
        by_id("expected_revision", Value::Null),
    )
    .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    fixture.stop().await;
    Ok(())
}

/// A fire on a console that requires sign-in records a thread whose replies
/// are for the schedule's sender (its caller scope, its console principal
/// and the requester the fire carried); the same schedule fired on an open
/// console names no one to reply to privately.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fire_thread_names_the_schedule_sender() -> TestResult {
    let fixture = delivering_fixture("console-schedules-fire-thread").await?;
    let controller = enforced(vec![grant(
        "erin-drives-alpha",
        ERIN,
        &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
        "alpha",
    )])?;
    let schedule_id = create_as(&fixture, Caller::human(ERIN).under(&controller)).await?;
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let signed_in = doors(
        &fixture,
        console_decisions(true, false, &[ERIN]),
        Some(controller.clone()),
        fences.clone(),
        ungoverned(),
    );
    let key = format!("schedule:{schedule_id}:occurrence:1");
    fire(&fixture, &signed_in, &schedule_id, &key).await?;
    let sent = sends_under(&fixture, "alpha", &key).await;
    assert_eq!(sent.len(), 1, "{sent:#?}");
    let interaction = sent[0].interaction_id.clone().expect("an interaction");
    let replies = crate::console_replies::ConsoleReplies::new(fixture.aggregator.clone(), None);
    let reference = replies
        .thread_reference(&interaction)
        .await?
        .expect("the fire records a thread");
    assert_eq!(
        reference.addressee,
        Some(super::console_caller_scope(&Caller::human(ERIN).scope())),
        "{reference:?}"
    );
    assert_eq!(reference.access_principal.as_deref(), Some(ERIN));
    assert_eq!(
        reference.requester,
        crate::runtime::console_human_principal(ISSUER, &subject_of(ERIN)).ok(),
        "{reference:?}"
    );
    assert_eq!(
        reference.audience,
        crate::console_aggregator::ConsoleThreadAudience::Shared
    );

    let open = doors(
        &fixture,
        open_decisions(false),
        Some(controller),
        fences,
        ungoverned(),
    );
    let open_key = format!("schedule:{schedule_id}:occurrence:2");
    fire(&fixture, &open, &schedule_id, &open_key).await?;
    let sent = sends_under(&fixture, "alpha", &open_key).await;
    assert_eq!(sent.len(), 1, "{sent:#?}");
    let interaction = sent[0].interaction_id.clone().expect("an interaction");
    let reference = replies
        .thread_reference(&interaction)
        .await?
        .expect("the fire records a thread");
    assert_eq!(
        (reference.addressee.clone(), reference.requester.clone()),
        (None, None),
        "{reference:?}"
    );
    assert_eq!(replies.can_deliver(&interaction).await, Ok(false));
    fixture.stop().await;
    Ok(())
}
