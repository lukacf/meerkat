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

/// A fire runs the Console's send gate as its sender, like a live send: a
/// fire to a member its sender may not know of, under rules that refuse the
/// sender a send to a name no member holds, is refused as the rules refuse
/// it, and nothing is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fire_to_a_member_its_sender_may_not_know_is_refused_as_the_rules_refuse_it() -> TestResult
{
    let fixture = delivering_fixture("console-schedules-fire-hidden").await?;
    // A member private to another person than Erin.
    fixture
        .runtime
        .handle()
        .spawn_spec(
            SpawnMemberSpec::new("agent", AgentIdentity::from("gamma")).with_labels(
                std::collections::BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "console-caller:v1:another-person".to_string(),
                )]),
            ),
        )
        .await?;
    // The schedule is Erin's all the same: defined through a door that
    // holds no audience.
    let created = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/create",
        create_params("gamma"),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    let schedule_id = created["result"]["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();
    let controller = enforced(vec![grant(
        "frank-drives-every-agent",
        FRANK,
        &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
        "*",
    )])?;
    let state_dir = tempfile::tempdir()?;
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())?;
    let writable = doors(
        &fixture,
        open_decisions(false),
        Some(controller),
        fences,
        ungoverned(),
    );

    let key = format!("schedule:{schedule_id}:occurrence:1");
    refused_because(
        fire(&fixture, &writable, &schedule_id, &key).await,
        "you are not allowed to send to this agent",
    );
    assert!(sends_under(&fixture, "gamma", &key).await.is_empty());
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

/// A gateway-shaped composition whose agents `scout` and `ledger` are created
/// at run time, with the schedule service a gateway attaches, the registry
/// its schedule tools judge targets with, and the family of changes of its
/// agents, whose archive, unarchive and removal the launch's schedules
/// follow.
struct CreatedAgents {
    fixture: Fixture,
    registry: crate::schedule_wiring::ScheduleMobTargetRegistry,
    family: Arc<dyn crate::identity_creation::IdentityFamily>,
    mob: meerkat_mob::MobHandle,
}

/// The host's roster: one configured agent, and none a row creates.
struct HostRoster;

#[async_trait::async_trait]
impl crate::identity_first::contracts::RosterProvider for HostRoster {
    async fn roster(
        &self,
        _context: &crate::identity_first::RosterContext,
    ) -> Result<Vec<crate::identity_first::DurableAgentSpec>, crate::identity_first::RosterError>
    {
        Ok(vec![crate::identity_first::DurableAgentSpec {
            identity: crate::identity_first::AgentIdentity::parse("host-a")
                .expect("a configured agent's name"),
            profile: "worker".into(),
            addressability: crate::identity_first::AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }])
    }
}

async fn created_agents(
    mob_id: &str,
) -> Result<CreatedAgents, Box<dyn std::error::Error + Send + Sync>> {
    use crate::identity_creation::{
        IdentityChangeSpec, IdentityCreationPolicy, IdentityFamilyOptions,
    };
    let temp_dir = tempfile::tempdir()?;
    let owner = Box::pin(
        crate::unified_runtime::UnifiedRuntimeBuilder::default()
            .definition(meerkat_mob::MobDefinition::from_toml(&format!(
                r#"
[mob]
id = "{mob_id}"
[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#
            ))?)
            .persistent_state(temp_dir.path())
            .continuity_from_state_dir(temp_dir.path())
            .await
            .map_err(|error| error.to_string())?
            .roster_provider(Arc::new(HostRoster))
            .identity_runtime_instance_id("console-schedules-created-agents")
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .map_err(|error| error.to_string())?;
    let runtime = owner.mob_runtime().clone();
    let identity_runtime = owner.identity_runtime_ref().cloned();
    let fixture = fixture_over(
        temp_dir,
        Some(owner),
        runtime,
        identity_runtime,
        Some(ScheduleFiringHostBinding::external_driver()),
    );
    let owner = fixture
        .owner
        .as_ref()
        .ok_or("the fixture keeps its owner")?;
    let registry = crate::schedule_wiring::ScheduleMobTargetRegistry::default();
    registry.adopt_mob_runtime(
        &fixture.runtime,
        &crate::member_key::DefaultMob::new(fixture.runtime.handle().mob_id().clone()),
    );
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive", "capability", "peers", "profile", "archive",
                       "remove", "adopt"],
            "requester": {"kind": "host"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .ok_or("the policy is enabled")?;
    let schedules =
        crate::schedule_wiring::MobAgentSchedules::new(fixture.service.clone(), registry.clone());
    assert!(
        owner.attach_identity_policy(
            Arc::new(policy),
            IdentityFamilyOptions::new().with_schedules(Arc::new(schedules)),
        ),
        "the runtime serves identities and takes the policy"
    );
    let family = owner.identity_family().ok_or("the family is attached")?;
    let mob = owner.mob_handle();
    let agents = CreatedAgents {
        fixture,
        registry,
        family,
        mob,
    };
    agents
        .change(
            "create",
            IdentityChangeSpec::Create(serde_json::from_value(json!({"agents": [
                {"name": "scout", "profile": "worker"},
                {"name": "ledger", "profile": "worker"},
            ]}))?),
        )
        .await;
    Ok(agents)
}

impl CreatedAgents {
    /// One change of the family as the host's request `request`, which is
    /// carried out: its outcome as a surface writes it.
    async fn change(
        &self,
        request: &str,
        change: crate::identity_creation::IdentityChangeSpec,
    ) -> Value {
        use crate::identity_creation::{ChangeRequester, IdentityChangeCall, OperationId};
        let host = ChangeRequester::host("ops");
        let operation = OperationId::for_request(&host.recorded(), request).expect("a request id");
        let outcome = self
            .family
            .change(&self.mob, &host, IdentityChangeCall::new(operation, change))
            .await;
        assert_eq!(outcome.token(), "done", "{outcome:?}");
        outcome.to_json()
    }

    /// The agent `name` at its current spec, as a change names it.
    async fn named(&self, name: &str) -> crate::identity_creation::AgentRef {
        use crate::identity_creation::{
            ChangeRequester, GetAgent, IdentityRead, IdentityReadAnswer,
        };
        match self
            .family
            .read(
                &self.mob,
                &ChangeRequester::host("ops"),
                IdentityRead::Get(GetAgent::new(name)),
            )
            .await
        {
            Ok(IdentityReadAnswer::Get(record)) => serde_json::from_value(json!({
                "name": name,
                "expected_spec_digest": record.agent.spec_digest.as_str(),
            }))
            .expect("a change of one agent as a surface writes it"),
            other => panic!("a get answers one agent: {other:?}"),
        }
    }

    /// Archive `name`: the `schedules` member of the answer.
    async fn archive(&self, request: &str, name: &str) -> Value {
        let agent = self.named(name).await;
        self.change(
            request,
            crate::identity_creation::IdentityChangeSpec::Archive(agent),
        )
        .await["agents"][0]["schedules"]
            .clone()
    }

    /// Unarchive `name`: the `schedules` member of the answer.
    async fn unarchive(&self, request: &str, name: &str) -> Value {
        let agent = self.named(name).await;
        self.change(
            request,
            crate::identity_creation::IdentityChangeSpec::Unarchive(agent),
        )
        .await["agents"][0]["schedules"]
            .clone()
    }

    /// The session Core's entry of `name` says its member is bound to.
    fn session_of(&self, name: &str) -> meerkat_core::types::SessionId {
        match self
            .mob
            .identity_entry(&AgentIdentity::from(name))
            .expect("Core's entry of the name is read")
        {
            meerkat_mob::IdentityEntryRead::Entry(entry) => match entry.standing {
                meerkat_mob::EntryStanding::Seated(member) => {
                    member.session_id.expect("the seat has a session")
                }
                other => panic!("{name} is seated: {other:?}"),
            },
            other => panic!("{name} is an entry: {other:?}"),
        }
    }

    async fn state(&self, schedule_id: &str) -> (meerkat::SchedulePhase, bool) {
        let schedule = self
            .fixture
            .service
            .get(&ScheduleId::parse(schedule_id).expect("a schedule id"))
            .await
            .expect("the schedule is read");
        (schedule.phase, schedule.paused_for_target())
    }

    async fn stop(self) {
        self.fixture.stop().await;
    }
}

/// The id of the schedule a Console write answered.
fn answered_id(answer: &Value) -> String {
    assert!(answer["error"].is_null(), "{answer:#?}");
    answer["result"]["schedule_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the schedule has an id: {answer:#?}"))
        .to_string()
}

/// The schedules `meta/schedule/list` answers for `identity`.
async fn listed_for(fixture: &Fixture, identity: &str) -> Vec<Value> {
    let listed = call(
        fixture,
        Caller::open_console(),
        "meta/schedule/list",
        json!({ "identity": identity }),
    )
    .await;
    listed["result"]["schedules"]
        .as_array()
        .unwrap_or_else(|| panic!("a list of schedules: {listed:#?}"))
        .clone()
}

/// A schedule's row says when its pause is its agent's: `paused_for_target`
/// is true on a schedule Core paused because its agent takes no deliveries,
/// and absent on an active schedule and on one somebody paused by name.
#[tokio::test]
async fn a_schedule_row_says_when_its_pause_is_its_agents() -> TestResult {
    let fixture = fixture(
        "console-schedules-whose-pause",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let create = || {
        call(
            &fixture,
            Caller::human(ERIN),
            "meta/schedule/create",
            create_params("alpha"),
        )
    };
    let for_agent = answered_id(&create().await);
    let by_name = answered_id(&create().await);
    let active = answered_id(&create().await);
    fixture
        .service
        .pause_for_target(&ScheduleId::parse(&for_agent)?)
        .await?;
    fixture.service.pause(&ScheduleId::parse(&by_name)?).await?;

    let row = |rows: &[Value], id: &str| {
        rows.iter()
            .find(|row| row["schedule_id"] == json!(id))
            .cloned()
            .unwrap_or_else(|| panic!("the schedule {id} is listed: {rows:#?}"))
    };
    let rows = listed_for(&fixture, "alpha").await;
    let paused = row(&rows, &for_agent);
    assert_eq!(paused["phase"], json!("paused"), "{paused:#?}");
    assert_eq!(paused["paused_for_target"], json!(true), "{paused:#?}");
    let held = row(&rows, &by_name);
    assert_eq!(held["phase"], json!("paused"), "{held:#?}");
    assert!(held.get("paused_for_target").is_none(), "{held:#?}");
    let running = row(&rows, &active);
    assert_eq!(running["phase"], json!("active"), "{running:#?}");
    assert!(running.get("paused_for_target").is_none(), "{running:#?}");

    // Resumed for its agent, the row no longer says so.
    fixture
        .service
        .resume_for_target(&ScheduleId::parse(&for_agent)?)
        .await?;
    let resumed = row(&listed_for(&fixture, "alpha").await, &for_agent);
    assert_eq!(resumed["phase"], json!("active"), "{resumed:#?}");
    assert!(resumed.get("paused_for_target").is_none(), "{resumed:#?}");
    fixture.stop().await;
    Ok(())
}

/// An archive pauses the Console's schedules of its agent, as it pauses the
/// agent's other schedules: the archive's answer lists them, their row says
/// the pause is the agent's, and the unarchive resumes them. Another
/// agent's Console schedule is left alone, and a pause by name is its
/// caller's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_archive_pauses_an_agents_console_schedules_and_their_row_says_so() -> TestResult {
    let agents = created_agents("console-schedules-archived-agent").await?;
    let open = Caller::open_console();
    let scouts = answered_id(
        &call(
            &agents.fixture,
            open,
            "meta/schedule/create",
            create_params("scout"),
        )
        .await,
    );
    let ledgers = answered_id(
        &call(
            &agents.fixture,
            open,
            "meta/schedule/create",
            create_params("ledger"),
        )
        .await,
    );

    let paused = agents.archive("archive-1", "scout").await;
    assert_eq!(
        (paused["effect"].clone(), paused["count"].clone()),
        (json!("paused"), json!(1)),
        "the archive pauses the agent's Console schedule: {paused:#?}"
    );
    assert_eq!(paused["schedules"][0]["schedule_id"], json!(scouts));
    let rows = listed_for(&agents.fixture, "scout").await;
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["phase"], json!("paused"), "{rows:#?}");
    assert_eq!(rows[0]["paused_for_target"], json!(true), "{rows:#?}");
    let others = listed_for(&agents.fixture, "ledger").await;
    assert_eq!(others[0]["phase"], json!("active"), "{others:#?}");
    assert!(others[0].get("paused_for_target").is_none(), "{others:#?}");

    // A pause by name is its caller's own.
    let by_name = call(
        &agents.fixture,
        open,
        "meta/schedule/pause",
        json!({ "identity": "ledger", "schedule_id": ledgers }),
    )
    .await;
    assert_eq!(by_name["result"]["phase"], json!("paused"), "{by_name:#?}");
    assert!(
        by_name["result"].get("paused_for_target").is_none(),
        "{by_name:#?}"
    );

    let resumed = agents.unarchive("unarchive-1", "scout").await;
    assert_eq!(
        (resumed["effect"].clone(), resumed["count"].clone()),
        (json!("resumed"), json!(1)),
        "{resumed:#?}"
    );
    let rows = listed_for(&agents.fixture, "scout").await;
    assert_eq!(rows[0]["phase"], json!("active"), "{rows:#?}");
    assert!(rows[0].get("paused_for_target").is_none(), "{rows:#?}");
    assert_eq!(
        agents.state(&ledgers).await,
        (meerkat::SchedulePhase::Paused, false),
        "another agent's pause by name stays"
    );
    agents.stop().await;
    Ok(())
}

/// A removal of an agent ends the schedules that name it: each is deleted,
/// and the change's answer says so. Another agent's schedules stay as they
/// are.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removal_ends_an_agents_schedules_and_its_answer_says_so() -> TestResult {
    let agents = created_agents("console-schedules-removed-agent").await?;
    let open = Caller::open_console();
    let scouts = answered_id(
        &call(
            &agents.fixture,
            open,
            "meta/schedule/create",
            create_params("scout"),
        )
        .await,
    );
    let ledgers = answered_id(
        &call(
            &agents.fixture,
            open,
            "meta/schedule/create",
            create_params("ledger"),
        )
        .await,
    );

    let agent = agents.named("scout").await;
    let ended = agents
        .change(
            "remove-1",
            crate::identity_creation::IdentityChangeSpec::Remove(agent),
        )
        .await["agents"][0]["schedules"]
        .clone();
    assert_eq!(
        (ended["effect"].clone(), ended["count"].clone()),
        (json!("ended"), json!(1)),
        "the removal ends the agent's schedule: {ended:#?}"
    );
    assert_eq!(ended["schedules"][0]["schedule_id"], json!(scouts));
    assert_eq!(
        agents.state(&scouts).await.0,
        meerkat::SchedulePhase::Deleted,
        "the removed agent's schedule is deleted"
    );
    assert_eq!(
        agents.state(&ledgers).await,
        (meerkat::SchedulePhase::Active, false),
        "another agent's schedule stays as it is"
    );
    agents.stop().await;
    Ok(())
}

/// The Console creates and changes no schedule for an archived agent: both
/// are refused as a send to it is (`identity_archived`), and nothing is
/// written. A resume by name stays what it was, and takes the schedule
/// over. Another agent's schedules are created as before, and so are the
/// agent's own once it is unarchived.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_console_writes_no_schedule_for_an_archived_agent() -> TestResult {
    let agents = created_agents("console-schedules-no-archived-target").await?;
    let open = Caller::open_console();
    let scouts = answered_id(
        &call(
            &agents.fixture,
            open,
            "meta/schedule/create",
            create_params("scout"),
        )
        .await,
    );
    agents.archive("archive-1", "scout").await;
    let archived = |answer: &Value| {
        assert_eq!(
            (
                answer["error"]["code"].clone(),
                answer["error"]["data"]["kind"].clone(),
                answer["error"]["data"]["identity"].clone(),
            ),
            (json!(-32603), json!("identity_archived"), json!("scout")),
            "{answer:#?}"
        );
    };

    let created = call(
        &agents.fixture,
        open,
        "meta/schedule/create",
        create_params("scout"),
    )
    .await;
    archived(&created);
    assert_eq!(
        listed_for(&agents.fixture, "scout").await.len(),
        1,
        "nothing was created"
    );
    let changed = call(
        &agents.fixture,
        open,
        "meta/schedule/update",
        json!({ "identity": "scout", "schedule_id": scouts, "content": "Changed meanwhile." }),
    )
    .await;
    archived(&changed);
    assert_eq!(
        stored_send(&agents.fixture, &scouts).await?.content,
        "Post the morning summary.",
        "nothing was written"
    );
    assert_eq!(
        agents.state(&scouts).await,
        (meerkat::SchedulePhase::Paused, true),
        "the pause is still the agent's"
    );

    // Another agent's schedule is created as before.
    let ledgers = call(
        &agents.fixture,
        open,
        "meta/schedule/create",
        create_params("ledger"),
    )
    .await;
    assert_eq!(ledgers["result"]["phase"], json!("active"), "{ledgers:#?}");

    // A resume by name stays what it was: it takes the schedule over.
    let resumed = call(
        &agents.fixture,
        open,
        "meta/schedule/resume",
        json!({ "identity": "scout", "schedule_id": scouts }),
    )
    .await;
    assert!(resumed["error"].is_null(), "{resumed:#?}");
    assert_eq!(resumed["result"]["phase"], json!("active"), "{resumed:#?}");
    assert!(
        resumed["result"].get("paused_for_target").is_none(),
        "{resumed:#?}"
    );

    // Unarchived, the agent takes a new schedule and a change again.
    agents.unarchive("unarchive-1", "scout").await;
    let again = call(
        &agents.fixture,
        open,
        "meta/schedule/create",
        create_params("scout"),
    )
    .await;
    assert_eq!(again["result"]["phase"], json!("active"), "{again:#?}");
    let changed = call(
        &agents.fixture,
        open,
        "meta/schedule/update",
        json!({ "identity": "scout", "schedule_id": scouts, "content": "Changed after." }),
    )
    .await;
    assert!(changed["error"].is_null(), "{changed:#?}");
    agents.stop().await;
    Ok(())
}

/// A Console schedule whose create read its agent before the agent's
/// archive and wrote after the archive had listed the agent's schedules is
/// paused for the agent all the same: the create reads the agent again
/// after its write. Its answer shows the schedule as that left it, and the
/// unarchive resumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_console_schedule_created_across_its_agents_archive_is_paused_for_it() -> TestResult {
    let agents = created_agents("console-schedules-create-across-archive").await?;
    let hold = crate::console_schedules::ScheduleWriteHold::default();
    agents
        .fixture
        .runtime
        .console_schedules()
        .attached()
        .ok_or("the schedules are attached")?
        .hold_writes(hold.clone());
    let create = call(
        &agents.fixture,
        Caller::open_console(),
        "meta/schedule/create",
        create_params("scout"),
    );
    let archive = async {
        tokio::time::timeout(Duration::from_secs(30), hold.reached())
            .await
            .expect("the create reached the hold");
        let paused = agents.archive("archive-1", "scout").await;
        hold.release();
        paused
    };
    let (created, paused) = tokio::join!(create, archive);
    assert_eq!(
        paused["count"],
        json!(0),
        "the archive listed no schedule yet: {paused:#?}"
    );
    let scouts = answered_id(&created);
    assert_eq!(created["result"]["phase"], json!("paused"), "{created:#?}");
    assert_eq!(
        created["result"]["paused_for_target"],
        json!(true),
        "{created:#?}"
    );
    assert_eq!(
        agents.state(&scouts).await,
        (meerkat::SchedulePhase::Paused, true)
    );
    let resumed = agents.unarchive("unarchive-1", "scout").await;
    assert_eq!(resumed["count"], json!(1), "{resumed:#?}");
    assert_eq!(
        agents.state(&scouts).await,
        (meerkat::SchedulePhase::Active, false)
    );
    agents.stop().await;
    Ok(())
}

/// One agent schedule tool call `name` with `args` through `tools`: the
/// JSON it answers, or its refusal.
async fn schedule_tool_call(
    tools: &Arc<dyn meerkat_core::AgentToolDispatcher>,
    name: &str,
    args: Value,
) -> Result<Value, meerkat_core::ToolError> {
    schedule_tool_call_in(
        tools,
        &meerkat_core::ToolDispatchContext::default(),
        name,
        args,
    )
    .await
}

/// A schedule tool call as the turn `context` describes makes it.
async fn schedule_tool_call_in(
    tools: &Arc<dyn meerkat_core::AgentToolDispatcher>,
    context: &meerkat_core::ToolDispatchContext,
    name: &str,
    args: Value,
) -> Result<Value, meerkat_core::ToolError> {
    let args = serde_json::value::RawValue::from_string(args.to_string())
        .map_err(|error| meerkat_core::ToolError::invalid_arguments(name, error.to_string()))?;
    let outcome = tools
        .dispatch_with_context(
            meerkat_core::types::ToolCallView {
                id: "schedule-tool-call",
                name,
                args: &args,
            },
            context,
        )
        .await?;
    let text = outcome
        .result
        .content
        .iter()
        .filter_map(|block| match block {
            meerkat_core::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

/// A daily schedule named `name` at `target`, as a schedule tool takes it.
fn tool_schedule(name: &str, target: Value) -> Value {
    json!({
        "name": name,
        "trigger": {
            "type": "interval",
            "start_at_utc": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            "every_seconds": 86400,
        },
        "target": target,
    })
}

/// An identity target that names `agent`.
fn identity_tool_target(agent: &str) -> Value {
    json!({
        "target_kind": "identity",
        "type": "resumable_identity",
        "identity": agent,
        "action": { "type": "prompt", "prompt": "Check in." },
    })
}

/// A member target that names `member` of the mob `mob`.
fn member_tool_target(mob: &str, member: &str) -> Value {
    json!({
        "target_kind": "mob",
        "type": "member",
        "mob_id": mob,
        "member_id": member,
        "action": { "type": "send", "content": "Check in." },
    })
}

/// An agent's schedule tools point no schedule at an archived agent: a
/// create whose target names it, an update that retargets a schedule at it
/// and an update of a schedule that already targets it are refused
/// (`identity_archived`) and write nothing. A schedule for another agent,
/// an update that moves a schedule away from the archived agent, and a
/// schedule for the agent once it is unarchived are taken as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_schedule_tools_point_no_schedule_at_an_archived_agent() -> TestResult {
    let agents = created_agents("schedule-tools-no-archived-target").await?;
    let mob = agents.mob.mob_id().to_string();
    let service = agents.fixture.service.clone();
    let tools = crate::schedule_wiring::agent_schedule_tools_over_registry_for_test(
        service.clone(),
        agents.registry.clone(),
    );
    let id_of = |created: &Value| {
        created["schedule_id"]
            .as_str()
            .unwrap_or_else(|| panic!("the schedule has an id: {created:#?}"))
            .to_string()
    };
    let scouts = id_of(
        &schedule_tool_call(
            &tools,
            "meerkat_schedule_create",
            tool_schedule("scouts", identity_tool_target("scout")),
        )
        .await?,
    );
    let ledgers = id_of(
        &schedule_tool_call(
            &tools,
            "meerkat_schedule_create",
            tool_schedule("ledgers", identity_tool_target("ledger")),
        )
        .await?,
    );
    agents.archive("archive-1", "scout").await;
    let archived = |answer: &Result<Value, meerkat_core::ToolError>| {
        assert!(
            matches!(
                answer,
                Err(meerkat_core::ToolError::PolicyDenied { denial })
                    if denial.code == "identity_archived" && denial.message.contains("scout")
            ),
            "refused because its agent is archived: {answer:?}"
        );
    };
    let count = service.list().await?.len();

    for target in [
        identity_tool_target("scout"),
        member_tool_target(&mob, "scout"),
    ] {
        let created = schedule_tool_call(
            &tools,
            "meerkat_schedule_create",
            tool_schedule("late", target),
        )
        .await;
        archived(&created);
    }
    assert_eq!(service.list().await?.len(), count, "nothing was created");

    let before = service.get(&ScheduleId::parse(&ledgers)?).await?;
    let retargeted = schedule_tool_call(
        &tools,
        "meerkat_schedule_update",
        json!({ "schedule_id": ledgers, "target": identity_tool_target("scout") }),
    )
    .await;
    archived(&retargeted);
    let renamed = schedule_tool_call(
        &tools,
        "meerkat_schedule_update",
        json!({ "schedule_id": scouts, "name": "renamed" }),
    )
    .await;
    archived(&renamed);
    assert_eq!(
        service.get(&ScheduleId::parse(&ledgers)?).await?,
        before,
        "the other agent's schedule is as it was"
    );
    assert_eq!(
        service
            .get(&ScheduleId::parse(&scouts)?)
            .await?
            .config
            .name
            .as_deref(),
        Some("scouts"),
        "nothing was written"
    );

    // Another agent's schedule, and a move away from the archived agent.
    schedule_tool_call(
        &tools,
        "meerkat_schedule_create",
        tool_schedule("more", identity_tool_target("ledger")),
    )
    .await?;
    schedule_tool_call(
        &tools,
        "meerkat_schedule_update",
        json!({ "schedule_id": scouts, "target": identity_tool_target("ledger") }),
    )
    .await?;

    agents.unarchive("unarchive-1", "scout").await;
    schedule_tool_call(
        &tools,
        "meerkat_schedule_create",
        tool_schedule("again", identity_tool_target("scout")),
    )
    .await?;
    agents.stop().await;
    Ok(())
}

/// An agent's own turn that names an archived agent it is not wired to is
/// answered as its turn that names an agent nobody holds: the same code
/// (`schedule_target_not_a_peer`) and the same sentence, by a create and by
/// an update that retargets its own schedule, and nothing is written. The
/// answer does not say that the name is an archived agent's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_archived_agent_out_of_an_agents_reach_answers_as_a_name_no_agent_holds() -> TestResult {
    let agents = created_agents("schedule-tools-archived-out-of-reach").await?;
    let mob = agents.mob.mob_id().to_string();
    let service = agents.fixture.service.clone();
    let tools = crate::schedule_wiring::agent_schedule_tools_over_registry_for_test(
        service.clone(),
        agents.registry.clone(),
    );
    let ledger_identity = AgentIdentity::from("ledger");
    agents.mob.ensure_member_live(&ledger_identity).await?;
    let ledger = agents
        .mob
        .get_member(&ledger_identity)
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or("ledger has a session")?;
    let as_ledger =
        meerkat_core::ToolDispatchContext::default().with_runtime_identity(ledger, None);

    // The agent's own schedule, for itself, written by its own turn.
    let own = schedule_tool_call_in(
        &tools,
        &as_ledger,
        "meerkat_schedule_create",
        tool_schedule("own", member_tool_target(&mob, "ledger")),
    )
    .await
    .map_err(|error| format!("an agent schedules for itself: {error:?}"))?;
    let own = own["schedule_id"]
        .as_str()
        .ok_or("the schedule has an id")?
        .to_string();
    agents.archive("archive-1", "scout").await;
    let before = service.list().await?;

    let mut created = Vec::new();
    let mut retargeted = Vec::new();
    for member in ["scout", "nobody"] {
        for (name, args, answers) in [
            (
                "meerkat_schedule_create",
                tool_schedule("late", member_tool_target(&mob, member)),
                &mut created,
            ),
            (
                "meerkat_schedule_update",
                json!({ "schedule_id": own, "target": member_tool_target(&mob, member) }),
                &mut retargeted,
            ),
        ] {
            let answer = schedule_tool_call_in(&tools, &as_ledger, name, args)
                .await
                .err();
            assert!(
                matches!(
                    &answer,
                    Some(meerkat_core::ToolError::PolicyDenied { denial })
                        if denial.code == meerkat::SCHEDULE_TARGET_NOT_A_PEER
                ),
                "{name} for {member}, out of the agent's reach, is refused as one: {answer:?}"
            );
            answers.push(format!("{answer:?}").replace(member, "<member>"));
        }
    }
    assert_eq!(
        created[0], created[1],
        "a create for an archived agent out of reach answers as one for a name no agent holds"
    );
    assert_eq!(
        retargeted[0], retargeted[1],
        "an update to an archived agent out of reach answers as one to a name no agent holds"
    );
    assert_eq!(service.list().await?, before, "nothing was written");
    agents.stop().await;
    Ok(())
}

/// A schedule that names an agent's session follows the agent's archive and
/// unarchive as a schedule that names the agent does: Core's entry of the
/// agent records the session its member was bound to, and its unarchive
/// seats it on that session again. Another agent's session schedule is left
/// alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_schedule_that_names_an_agents_session_follows_its_archive_and_unarchive() -> TestResult {
    use meerkat::SchedulePhase::{Active, Paused};
    let agents = created_agents("schedules-session-target-archived").await?;
    let on_session = |name: &'static str, agent: &'static str| {
        let session_id = agents.session_of(agent);
        let service = agents.fixture.service.clone();
        async move {
            service
                .create(meerkat::CreateScheduleRequest {
                    name: Some(name.to_owned()),
                    description: None,
                    trigger: meerkat::TriggerSpec::Once {
                        due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
                    },
                    target: meerkat::TargetBinding::session(
                        meerkat::SessionTargetBinding::ExactSession {
                            session_id,
                            action: meerkat::ScheduledSessionAction::Prompt {
                                prompt: meerkat_core::ContentInput::Text("Check in.".into()),
                                system_prompt: None,
                                render_metadata: None,
                                skill_refs: Vec::new(),
                                additional_instructions: Vec::new(),
                            },
                        },
                    ),
                    misfire_policy: meerkat::MisfirePolicy::default(),
                    overlap_policy: meerkat::OverlapPolicy::default(),
                    missing_target_policy: meerkat::MissingTargetPolicy::default(),
                    labels: BTreeMap::new(),
                    planning_horizon_days: Some(1),
                    planning_horizon_occurrences: Some(1),
                })
                .await
                .unwrap_or_else(|error| panic!("the schedule {name} is created: {error}"))
                .schedule_id
                .to_string()
        }
    };
    let scouts = on_session("scouts", "scout").await;
    let ledgers = on_session("ledgers", "ledger").await;

    let paused = agents.archive("archive-1", "scout").await;
    assert_eq!(
        (paused["effect"].clone(), paused["count"].clone()),
        (json!("paused"), json!(1)),
        "the archive pauses the schedule on the agent's session: {paused:#?}"
    );
    assert_eq!(agents.state(&scouts).await, (Paused, true));
    assert_eq!(agents.state(&ledgers).await, (Active, false));

    let resumed = agents.unarchive("unarchive-1", "scout").await;
    assert_eq!(
        (resumed["effect"].clone(), resumed["count"].clone()),
        (json!("resumed"), json!(1)),
        "{resumed:#?}"
    );
    assert_eq!(agents.state(&scouts).await, (Active, false));
    assert_eq!(agents.state(&ledgers).await, (Active, false));
    agents.stop().await;
    Ok(())
}

/// A schedule that is no Console schedule and whose target names an agent (a
/// member target, as the agents' schedule tools and a host write one) is
/// listed on that agent's view, beside its Console schedules and never among
/// them, and on no other agent's view.
#[tokio::test]
async fn the_console_lists_a_schedule_another_writer_aimed_at_the_agent() -> TestResult {
    let fixture = fixture(
        "console-other-schedules",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let mob = fixture.runtime.handle().mob_id().to_string();
    let schedule = fixture
        .service
        .create(meerkat::CreateScheduleRequest {
            name: Some("a note".to_owned()),
            description: None,
            trigger: meerkat::TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: meerkat::TargetBinding::Mob(Box::new(meerkat::MobTargetBinding::Member {
                mob_id: mob,
                member_id: "beta".to_string(),
                action: meerkat::ScheduledMobAction::Send {
                    content: meerkat_core::ContentInput::Text("Say hello.".into()),
                    render_metadata: None,
                },
            })),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await
        .map_err(|error| error.to_string())?
        .schedule_id;

    let listed = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/list",
        json!({ "identity": "beta" }),
    )
    .await;
    assert_eq!(
        listed["result"]["schedules"],
        json!([]),
        "it is no Console schedule: {listed:#?}"
    );
    let others = listed["result"]["other_schedules"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        others
            .iter()
            .map(|other| other["schedule_id"].clone())
            .collect::<Vec<_>>(),
        vec![json!(schedule.to_string())],
        "the agent's view lists the schedule aimed at it: {listed:#?}"
    );
    assert_eq!(others[0]["content"], json!("Say hello."), "{listed:#?}");

    let other_agent = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    assert_eq!(
        other_agent["result"]["other_schedules"],
        json!([]),
        "another agent's view does not list it: {other_agent:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A model that counts the requests it is sent and answers each with one
/// short turn.
struct CountedModel {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl meerkat_client::LlmClient for CountedModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>>
                + Send
                + 'a,
        >,
    > {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            meerkat_core::types::StopReason::EndTurn,
        );
        Box::pin(futures::stream::iter(vec![
            Ok(meerkat_client::LlmEvent::TextDelta {
                delta: "done".to_string(),
                meta: None,
            }),
            Ok(usage),
            Ok(done),
        ]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), meerkat_client::LlmError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// The agents of [`created_agents`] on a launch whose schedule firing host
/// runs: the same composition, built over a session service the row holds
/// (a firing host fires through it) and a model that counts its requests.
struct FiringAgents {
    agents: CreatedAgents,
    live: crate::mob_handle_runtime::LiveComposeInputs,
    model_calls: Arc<std::sync::atomic::AtomicUsize>,
}

/// What a launch holds of an agent's session.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionReading {
    /// The session's stored messages.
    messages: String,
    /// Whether a session actor lives for it.
    actor: bool,
    /// Whether an executor is attached to it.
    executor: bool,
    /// Where Core's entry says the agent stands.
    standing: String,
}

/// How one occurrence ended: its phase and its failure class.
type Ending = (
    meerkat::OccurrencePhase,
    Option<meerkat::OccurrenceFailureClass>,
);

async fn firing_agents(
    mob_id: &str,
) -> Result<FiringAgents, Box<dyn std::error::Error + Send + Sync>> {
    use crate::identity_creation::{
        IdentityChangeSpec, IdentityCreationPolicy, IdentityFamilyOptions,
    };
    let temp_dir = tempfile::tempdir()?;
    let state = temp_dir.path().to_path_buf();
    let model_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (mob_storage, provenance) = crate::mob_composition_manifest::persistent_mob_storage(
        crate::storage_layout::MetaStorageLayout::with_injected_roots(state.clone(), None)
            .event_log_db(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let mut spec = crate::mob_handle_runtime::MobBootstrapSpec::persistent(
        meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"
[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#
        ))?,
        mob_storage,
        provenance,
        state.clone(),
        16,
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    spec.options = crate::mob_handle_runtime::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(CountedModel {
            calls: Arc::clone(&model_calls),
        })),
    };
    let live = spec
        .live_compose_inputs
        .clone()
        .ok_or("a persistent composition keeps its session service")?;
    let owner = Box::pin(
        crate::unified_runtime::UnifiedRuntimeBuilder::default()
            .mob_spec(spec)
            .module_config(crate::MetaConfig {
                modules: Vec::new(),
                discovery: crate::DiscoverySpec {
                    namespace: mob_id.to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(Duration::from_secs(10))
            .persistent_state(state.clone())
            .continuity_from_state_dir(&state)
            .await
            .map_err(|error| error.to_string())?
            .roster_provider(Arc::new(HostRoster))
            .identity_runtime_instance_id("schedule-fires-created-agents")
            .build(),
    )
    .await
    .map_err(|error| error.to_string())?;
    let runtime = owner.mob_runtime().clone();
    let identity_runtime = owner.identity_runtime_ref().cloned();
    let fixture = fixture_over(
        temp_dir,
        Some(owner),
        runtime,
        identity_runtime,
        Some(ScheduleFiringHostBinding::external_driver()),
    );
    let owner = fixture
        .owner
        .as_ref()
        .ok_or("the fixture keeps its owner")?;
    let registry = crate::schedule_wiring::ScheduleMobTargetRegistry::default();
    registry.adopt_mob_runtime(
        &fixture.runtime,
        &crate::member_key::DefaultMob::new(fixture.runtime.handle().mob_id().clone()),
    );
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive", "capability", "peers", "profile", "archive",
                       "remove", "adopt"],
            "requester": {"kind": "host"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .ok_or("the policy is enabled")?;
    let schedules =
        crate::schedule_wiring::MobAgentSchedules::new(fixture.service.clone(), registry.clone());
    assert!(
        owner.attach_identity_policy(
            Arc::new(policy),
            IdentityFamilyOptions::new().with_schedules(Arc::new(schedules)),
        ),
        "the runtime serves identities and takes the policy"
    );
    let family = owner.identity_family().ok_or("the family is attached")?;
    let mob = owner.mob_handle();
    let agents = CreatedAgents {
        fixture,
        registry,
        family,
        mob,
    };
    agents
        .change(
            "create",
            IdentityChangeSpec::Create(serde_json::from_value(json!({"agents": [
                {"name": "scout", "profile": "worker"},
                {"name": "ledger", "profile": "worker"},
            ]}))?),
        )
        .await;
    Ok(FiringAgents {
        agents,
        live,
        model_calls,
    })
}

impl FiringAgents {
    /// The schedule firing host of the launch, over the composition's own
    /// session service, runtime machine, mob and identity runtime.
    fn start_firing(&self) -> impl Sized + use<> {
        let owner = self
            .agents
            .fixture
            .owner
            .as_ref()
            .expect("the fixture keeps its owner");
        crate::schedule_wiring::spawn_schedule_host_with_identity_runtime(
            Arc::clone(&self.live.service),
            Arc::clone(&self.live.machine),
            self.agents.fixture.service.clone(),
            owner.mob_runtime().agent_mob_mcp_state(),
            owner.mob_handle(),
            owner.identity_runtime_ref().cloned(),
            None,
            None,
            "schedule-fires-created-agents",
        )
        .expect("the schedule firing host starts")
    }

    /// A schedule that names `session` alone.
    fn on_session(session: &meerkat_core::types::SessionId) -> meerkat::TargetBinding {
        meerkat::TargetBinding::session(meerkat::SessionTargetBinding::ExactSession {
            session_id: session.clone(),
            action: meerkat::ScheduledSessionAction::Prompt {
                prompt: meerkat_core::ContentInput::Text("Check in.".into()),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        })
    }

    /// A schedule that names the agent `name` as a member of its mob, as the
    /// agents' schedule tools write one.
    fn on_agent(&self, name: &str) -> meerkat::TargetBinding {
        meerkat::TargetBinding::Mob(Box::new(meerkat::MobTargetBinding::Member {
            mob_id: self.agents.mob.definition().id.as_str().to_string(),
            member_id: name.to_string(),
            action: meerkat::ScheduledMobAction::Send {
                content: meerkat_core::ContentInput::Text("Say hello.".into()),
                render_metadata: None,
            },
        }))
    }

    /// A schedule on `target`, written to the schedule service as a host
    /// writes one.
    async fn schedule(
        &self,
        name: &str,
        target: meerkat::TargetBinding,
        trigger: meerkat::TriggerSpec,
    ) -> ScheduleId {
        self.agents
            .fixture
            .service
            .create(meerkat::CreateScheduleRequest {
                name: Some(name.to_owned()),
                description: None,
                trigger,
                target,
                // A late tick of a busy host is not what a row reads.
                misfire_policy: meerkat::MisfirePolicy::CatchUpWithin {
                    window_seconds: 120,
                },
                overlap_policy: meerkat::OverlapPolicy::default(),
                missing_target_policy: meerkat::MissingTargetPolicy::default(),
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(3),
            })
            .await
            .unwrap_or_else(|error| panic!("the schedule {name} is created: {error}"))
            .schedule_id
    }

    /// What the launch holds of `name`'s session `session`.
    async fn reading(
        &self,
        name: &str,
        session: &meerkat_core::types::SessionId,
    ) -> SessionReading {
        let head = |text: String| {
            text.split(|letter: char| !letter.is_alphanumeric())
                .next()
                .unwrap_or_default()
                .to_string()
        };
        SessionReading {
            messages: match self.live.service.load_authoritative_session(session).await {
                Ok(Some(stored)) => format!("{} messages", stored.messages().len()),
                Ok(None) => "no session".to_string(),
                Err(error) => format!("unreadable: {error}"),
            },
            actor: self
                .live
                .service
                .live_session_actor_witness(session)
                .await
                .is_some(),
            executor: self
                .live
                .machine
                .current_executor_attachment_witness(session)
                .await
                .is_some(),
            standing: match self.agents.mob.identity_entry(&AgentIdentity::from(name)) {
                Ok(meerkat_mob::IdentityEntryRead::Entry(entry)) => {
                    head(format!("{:?}", entry.standing))
                }
                Ok(other) => head(format!("{other:?}")),
                Err(error) => format!("unreadable: {error:?}"),
            },
        }
    }

    /// The first `count` occurrences of `schedule` that came due and ended,
    /// in due order: fewer when no more ended `within`.
    async fn ended(
        &self,
        schedule: &ScheduleId,
        count: usize,
        within: Duration,
    ) -> Vec<meerkat::Occurrence> {
        use meerkat::OccurrencePhase::{Completed, DeliveryFailed, Misfired, Skipped};
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let mut ended: Vec<_> = self
                .agents
                .fixture
                .service
                .list_occurrences(schedule)
                .await
                .expect("the occurrences are read")
                .into_iter()
                .filter(|occurrence| {
                    matches!(
                        occurrence.phase,
                        Completed | DeliveryFailed | Misfired | Skipped
                    )
                })
                .collect();
            ended.sort_by_key(|occurrence| occurrence.due_at_utc);
            if ended.len() >= count || tokio::time::Instant::now() >= deadline {
                ended.truncate(count);
                return ended;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// How `scout` is taken away before the schedule on its session comes due.
#[derive(Debug, Clone, Copy)]
enum TakenAway {
    /// The agent is removed. The removal finds no schedule on a session
    /// Core's entry no longer records, so the schedule stays active.
    Removed,
    /// The agent is archived, which pauses the schedule for its target, and
    /// the schedule is then resumed by name.
    ArchivedAndResumedByName,
}

/// What a launch whose firing host runs did with two schedules: a one-shot
/// on the session of `ledger`, which stays as it was created, and a
/// repeating one on the session of `scout`, which came due after `scout`
/// was taken away.
struct Fired {
    /// How each agent's first turn ended, the model requests they made, and
    /// what the launch held of `scout`'s session after its own.
    woken: Vec<Ending>,
    woken_calls: usize,
    scout_woken: SessionReading,
    /// How the one-shot on `ledger`'s session ended.
    control: Vec<Ending>,
    /// The model requests made by the time it ended.
    control_calls: usize,
    /// Whether `ledger`'s session holds more than before it.
    control_grew: bool,
    /// How the first occurrences of the schedule on `scout`'s session ended.
    ended: Vec<Ending>,
    /// The model requests made by the time they ended.
    calls: usize,
    /// `scout`'s session before the schedule came due, and after.
    before: SessionReading,
    after: SessionReading,
    /// The schedule on `scout`'s session afterwards: its phase, and whether
    /// it is paused for its target.
    schedule: (meerkat::SchedulePhase, bool),
}

async fn due_after(
    mob_id: &str,
    taken: TakenAway,
) -> Result<Fired, Box<dyn std::error::Error + Send + Sync>> {
    use meerkat::SchedulePhase::{Active, Paused};
    use std::sync::atomic::Ordering::SeqCst;
    let fired = firing_agents(mob_id).await?;
    let host = fired.start_firing();
    // An agent the create seated has no stored session before its first
    // turn, and a schedule on a session that is not stored ends as a missing
    // target. Each agent takes one turn first, through a schedule that names
    // it as the agents' schedule tools name one.
    let soon = chrono::Utc::now() + chrono::Duration::seconds(2);
    let mut wakes = Vec::new();
    for name in ["scout", "ledger"] {
        wakes.push(
            fired
                .schedule(
                    &format!("wake-{name}"),
                    fired.on_agent(name),
                    meerkat::TriggerSpec::Once { due_at_utc: soon },
                )
                .await,
        );
    }
    let mut woken = Vec::new();
    for wake in &wakes {
        woken.extend(fired.ended(wake, 1, Duration::from_mins(1)).await);
    }
    let first_turns = tokio::time::Instant::now() + Duration::from_mins(1);
    while fired.model_calls.load(SeqCst) < 2 && tokio::time::Instant::now() < first_turns {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The first turns' writes land.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let woken_calls = fired.model_calls.load(SeqCst);
    let scout = fired.agents.session_of("scout");
    let ledger = fired.agents.session_of("ledger");
    let scout_woken = fired.reading("scout", &scout).await;
    let created = chrono::Utc::now();
    let control_due = created + chrono::Duration::seconds(20);
    let control = fired
        .schedule(
            "ledgers",
            FiringAgents::on_session(&ledger),
            meerkat::TriggerSpec::Once {
                due_at_utc: control_due,
            },
        )
        .await;
    let scouts = fired
        .schedule(
            "scouts",
            FiringAgents::on_session(&scout),
            meerkat::TriggerSpec::Interval(meerkat::IntervalTriggerSpec {
                start_at_utc: created + chrono::Duration::seconds(26),
                every_seconds: 5,
                end_at_utc: None,
            }),
        )
        .await;
    match taken {
        TakenAway::Removed => {
            let agent = fired.agents.named("scout").await;
            fired
                .agents
                .change(
                    "remove-1",
                    crate::identity_creation::IdentityChangeSpec::Remove(agent),
                )
                .await;
        }
        TakenAway::ArchivedAndResumedByName => {
            fired.agents.archive("archive-1", "scout").await;
            assert_eq!(
                fired.agents.state(&scouts.to_string()).await,
                (Paused, true),
                "the archive pauses the schedule on the agent's session"
            );
            fired
                .agents
                .fixture
                .service
                .resume(&scouts)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    assert_eq!(
        fired.agents.state(&scouts.to_string()).await,
        (Active, false),
        "the schedule on the agent's session is active when it comes due"
    );
    assert!(
        chrono::Utc::now() < control_due,
        "the agent was taken away before any schedule came due"
    );
    assert_eq!(
        fired.model_calls.load(SeqCst),
        woken_calls,
        "no turn ran between the first turns and a schedule coming due"
    );
    let before = fired.reading("scout", &scout).await;
    let ledger_before = fired.reading("ledger", &ledger).await;

    let control_ended = fired.ended(&control, 1, Duration::from_secs(80)).await;
    let control_calls = fired.model_calls.load(SeqCst);
    let ledger_after = fired.reading("ledger", &ledger).await;

    let ended = fired.ended(&scouts, 2, Duration::from_mins(1)).await;
    // Whatever a fire started has shown by now.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let calls = fired.model_calls.load(SeqCst);
    let after = fired.reading("scout", &scout).await;
    let schedule = fired.agents.state(&scouts.to_string()).await;
    let endings = |occurrences: Vec<meerkat::Occurrence>| -> Vec<Ending> {
        occurrences
            .into_iter()
            .map(|occurrence| (occurrence.phase, occurrence.failure_class))
            .collect()
    };
    drop(host);
    let seen = Fired {
        woken: endings(woken),
        woken_calls,
        scout_woken,
        control: endings(control_ended),
        control_calls,
        control_grew: ledger_after.messages != ledger_before.messages,
        ended: endings(ended),
        calls,
        before,
        after,
        schedule,
    };
    fired.agents.stop().await;
    Ok(seen)
}

/// A schedule that names only the session of an agent comes due after the
/// agent's removal, on a launch whose firing host runs: nothing runs for
/// it. A schedule on another agent's session fires its turn on the same
/// host; the removed agent's takes no model request, its session holds
/// what it held, no session actor or executor appears for it, and each of
/// its occurrences ends without a delivery while the schedule stays active.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_schedule_that_comes_due_after_its_agents_removal_runs_nothing() -> TestResult {
    use meerkat::OccurrenceFailureClass::TargetMissing;
    use meerkat::OccurrencePhase::{Completed, Misfired};
    let fired = due_after("schedule-fires-removed-agent", TakenAway::Removed).await?;
    assert_eq!(
        (fired.woken.as_slice(), fired.woken_calls),
        ([(Completed, None), (Completed, None)].as_slice(), 2),
        "each agent took its first turn: {:?}",
        fired.scout_woken
    );
    assert_eq!(
        fired.control,
        [(Completed, None)],
        "the firing host delivers to a session of the launch"
    );
    assert!(
        fired.control_calls > fired.woken_calls && fired.control_grew,
        "the delivery to the other agent's session ran its turn"
    );
    assert_eq!(
        fired.calls, fired.control_calls,
        "no model request is made for the removed agent's session"
    );
    assert_eq!(
        fired.after, fired.before,
        "the removed agent's session holds what it held, and nothing woke or was built for it"
    );
    assert_eq!(
        fired.ended,
        [
            (Misfired, Some(TargetMissing)),
            (Misfired, Some(TargetMissing))
        ],
        "each occurrence ends without a delivery"
    );
    assert_eq!(
        fired.schedule,
        (meerkat::SchedulePhase::Active, false),
        "the schedule stays active"
    );
    Ok(())
}

/// A schedule that names only the session of an agent is paused by the
/// agent's archive and resumed by name while the agent is archived; it then
/// comes due on a launch whose firing host runs: nothing runs for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_schedule_resumed_by_name_for_an_archived_agent_runs_nothing() -> TestResult {
    use meerkat::OccurrenceFailureClass::TargetMissing;
    use meerkat::OccurrencePhase::{Completed, Misfired};
    let fired = due_after(
        "schedule-fires-archived-agent",
        TakenAway::ArchivedAndResumedByName,
    )
    .await?;
    assert_eq!(
        (fired.woken.as_slice(), fired.woken_calls),
        ([(Completed, None), (Completed, None)].as_slice(), 2),
        "each agent took its first turn: {:?}",
        fired.scout_woken
    );
    assert_eq!(
        fired.control,
        [(Completed, None)],
        "the firing host delivers to a session of the launch"
    );
    assert!(
        fired.control_calls > fired.woken_calls && fired.control_grew,
        "the delivery to the other agent's session ran its turn"
    );
    assert_eq!(
        fired.calls, fired.control_calls,
        "no model request is made for the archived agent's session"
    );
    assert_eq!(
        fired.after, fired.before,
        "the archived agent's session holds what it held, and nothing woke or was built for it"
    );
    assert_eq!(
        fired.ended,
        [
            (Misfired, Some(TargetMissing)),
            (Misfired, Some(TargetMissing))
        ],
        "each occurrence ends without a delivery"
    );
    assert_eq!(
        fired.schedule,
        (meerkat::SchedulePhase::Active, false),
        "the schedule stays active"
    );
    Ok(())
}

/// A console schedule is one agent's: its send names the agent by its mob
/// and its identity. A schedule of the agent `alpha` of another mob is not
/// this mob's `alpha`'s: it is not listed for it, it is answered by its id
/// as another agent's schedule is, and its fire reaches no agent here, while
/// this mob's own schedule lists and fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_of_another_mobs_agent_is_never_this_mobs() -> TestResult {
    let fixture = delivering_fixture("console-schedules-member-key").await?;
    let default_mob = crate::member_key::DefaultMob::new(fixture.runtime.handle().mob_id().clone());
    // As a write for the agent `alpha` of the mob `second` stores it.
    let other = ConsoleScheduledSend::new(
        &default_mob,
        &meerkat_mob::MobId::from("second"),
        "alpha".to_string(),
        "The other mob's summary.".to_string(),
        ConsoleScheduleSender::Anonymous,
        100,
    );
    let other_id = fixture
        .service
        .create(meerkat::CreateScheduleRequest {
            name: None,
            description: None,
            trigger: serde_json::from_value(hourly())?,
            target: other.target()?,
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: std::collections::BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await?
        .schedule_id
        .to_string();
    let own_id = create_as(&fixture, Caller::open_console()).await?;

    let listed = call(
        &fixture,
        Caller::open_console(),
        "meta/schedule/list",
        json!({ "identity": "alpha" }),
    )
    .await;
    let ids = listed["result"]["schedules"]
        .as_array()
        .ok_or("the list answers schedules")?
        .iter()
        .filter_map(|schedule| schedule["schedule_id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, [own_id.as_str()], "{listed:#?}");

    let pause = |identity: &'static str, schedule_id: String| {
        call(
            &fixture,
            Caller::open_console(),
            "meta/schedule/pause",
            json!({ "identity": identity, "schedule_id": schedule_id }),
        )
    };
    let of_another_mob = pause("alpha", other_id.clone()).await;
    let of_another_agent = pause("beta", own_id.clone()).await;
    assert!(!of_another_mob["error"].is_null(), "{of_another_mob:#?}");
    assert_eq!(
        error_of(&of_another_mob),
        error_of(&of_another_agent),
        "a schedule of the agent of that name in another mob is another agent's"
    );
    assert_eq!(
        fixture
            .service
            .get(&ScheduleId::parse(&other_id)?)
            .await?
            .phase,
        meerkat::SchedulePhase::Active,
        "the refused pause changed nothing"
    );

    let open = doors(
        &fixture,
        open_decisions(false),
        None,
        crate::auth::session_fence::ConsoleSessionFences::detached(),
        ungoverned(),
    );
    refused_because(
        fire(&fixture, &open, &other_id, "key-of-the-other-mob").await,
        "a member of mob second",
    );
    assert!(
        sends_under(&fixture, "alpha", "key-of-the-other-mob")
            .await
            .is_empty(),
        "nothing was sent to this mob's agent of the same name"
    );
    fire(&fixture, &open, &own_id, "key-of-this-mob").await?;
    assert_eq!(
        sends_under(&fixture, "alpha", "key-of-this-mob")
            .await
            .len(),
        1
    );
    fixture.stop().await;
    Ok(())
}

/// One host with two mobs: its launch mob, which is the host's default mob,
/// and a second mob, each with the agents `alpha` and `beta`. Both mobs'
/// console schedules live in the host's one schedule service.
struct TwoMobHost {
    launch: Fixture,
    second: Fixture,
    /// The host's default mob, as the host states it: its launch mob.
    default_mob: crate::member_key::DefaultMob,
}

/// A [`TwoMobHost`] whose mob ids start with `name`. With `delivering`, the
/// launch mob's agents take console sends.
async fn two_mob_host(
    name: &str,
    delivering: bool,
) -> Result<TwoMobHost, Box<dyn std::error::Error + Send + Sync>> {
    let launch_id = format!("{name}-launch");
    let launch = if delivering {
        delivering_fixture(&launch_id).await?
    } else {
        fixture(
            &launch_id,
            Some(ScheduleFiringHostBinding::external_driver()),
        )
        .await?
    };
    let second = fixture(&format!("{name}-second"), None).await?;
    second.runtime.console_schedules().attach(
        launch.service.clone(),
        ScheduleFiringHostBinding::external_driver(),
    );
    let default_mob = crate::member_key::DefaultMob::new(launch.runtime.handle().mob_id().clone());
    Ok(TwoMobHost {
        launch,
        second,
        default_mob,
    })
}

/// A [`TwoMobHost`] whose second mob has an identity plane of its own
/// ([`identity_plane_fixture`]) that knows `agents`, each with whether it
/// takes console sends. Its launch mob is a light console runtime.
async fn two_mob_host_with_an_identity_plane(
    name: &str,
    agents: &[(&str, bool)],
) -> Result<TwoMobHost, Box<dyn std::error::Error + Send + Sync>> {
    let launch = fixture(
        &format!("{name}-launch"),
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let second = identity_plane_fixture(&format!("{name}-second"), agents).await?;
    second.runtime.console_schedules().attach(
        launch.service.clone(),
        ScheduleFiringHostBinding::external_driver(),
    );
    let default_mob = crate::member_key::DefaultMob::new(launch.runtime.handle().mob_id().clone());
    Ok(TwoMobHost {
        launch,
        second,
        default_mob,
    })
}

/// A console runtime with an identity plane of its own, as an
/// identity-first gateway builds one: its identity runtime knows `agents`
/// (each with whether it takes console sends) and no other name. A request
/// the console's door resolves to this mob is handed that identity runtime.
async fn identity_plane_fixture(
    mob_id: &str,
    agents: &[(&str, bool)],
) -> Result<Fixture, Box<dyn std::error::Error + Send + Sync>> {
    use crate::identity_first::{
        AgentAddressability, AgentIdentity, DurableAgentSpec, MutableRosterProvider,
    };
    let temp_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let mut roster = Vec::with_capacity(agents.len());
    for (identity, takes_sends) in agents {
        roster.push(DurableAgentSpec {
            identity: AgentIdentity::parse(identity).map_err(|error| format!("{error:?}"))?,
            profile: "agent".into(),
            addressability: if *takes_sends {
                AgentAddressability::Addressable
            } else {
                AgentAddressability::InternalOnly
            },
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
            backend: None,
            binding: None,
            placement: None,
        });
    }
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
        .roster_provider(Arc::new(MutableRosterProvider::new(roster)))
        .identity_runtime_instance_id(format!("{mob_id}-instance"))
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .map_err(|error| error.to_string())?;
    let identity_runtime = owner
        .identity_runtime_ref()
        .cloned()
        .ok_or("a runtime built with a roster has an identity runtime")?;
    let runtime = owner.mob_runtime().clone();
    Ok(fixture_over(
        temp_dir,
        Some(owner),
        runtime,
        Some(identity_runtime),
        Some(ScheduleFiringHostBinding::external_driver()),
    ))
}

/// A schedule of the agent `alpha` of the mob `mob_id` in the schedule
/// service `service` of a host whose default mob is `default_mob`, stored
/// as a create for that agent stores it. The door refuses the create for
/// an agent of a mob no fire reaches, so the rows that read such a schedule
/// store it themselves.
async fn stored_for_alpha_of(
    service: &ScheduleService,
    default_mob: &crate::member_key::DefaultMob,
    mob_id: &meerkat_mob::MobId,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let send = ConsoleScheduledSend::new(
        default_mob,
        mob_id,
        "alpha".to_string(),
        "Post the morning summary.".to_string(),
        ConsoleScheduleSender::Anonymous,
        100,
    );
    Ok(service
        .create(meerkat::CreateScheduleRequest {
            name: Some("Morning summary".to_string()),
            description: None,
            trigger: serde_json::from_value(hourly())?,
            target: send.target()?,
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: std::collections::BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await?
        .schedule_id
        .to_string())
}

/// `method` with `params` at the schedule door, as the open Console's shared
/// account, for the mob `of` wraps, on a host whose default mob is
/// `default_mob` and whose console reads its agents from `aggregator`.
async fn at_the_schedule_door(
    default_mob: &crate::member_key::DefaultMob,
    of: &Fixture,
    aggregator: &MetaConsoleAggregator,
    method: &str,
    params: Value,
) -> Value {
    let governance = ConsoleSendGovernance {
        scope: ConsoleSendScope::Anonymous,
        ..ConsoleSendGovernance::default()
    };
    Box::pin(schedules::handle_for(
        schedules::ScheduleMob::on_host(default_mob, &of.runtime),
        &request(method, params),
        of.identity_runtime.as_ref(),
        Some(aggregator),
        None,
        None,
        &governance,
    ))
    .await
}

impl TwoMobHost {
    /// `method` with `params` at the schedule door, as the open Console's
    /// shared account, for the mob `of` wraps: the call of a request the
    /// console's door resolved to that mob, on this host. The door is handed
    /// what the console's dispatch hands it: the resolved mob's wrapper and
    /// identity runtime, and the console's one aggregator, which is built
    /// over the launch mob.
    async fn at(&self, of: &Fixture, method: &str, params: Value) -> Value {
        at_the_schedule_door(
            &self.default_mob,
            of,
            &self.launch.aggregator,
            method,
            params,
        )
        .await
    }

    /// A schedule created for the agent `alpha` of the mob `of` wraps.
    async fn created_for_alpha(&self, of: &Fixture) -> String {
        answered_id(
            &self
                .at(of, "meta/schedule/create", create_params("alpha"))
                .await,
        )
    }

    /// A schedule of the second mob's `alpha` in the host's schedule
    /// service ([`stored_for_alpha_of`]).
    async fn stored_for_alpha_of_the_second_mob(
        &self,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        stored_for_alpha_of(
            &self.launch.service,
            &self.default_mob,
            self.second.runtime.handle().mob_id(),
        )
        .await
    }

    /// The ids of the schedules the door lists for `alpha` of the mob `of`
    /// wraps.
    async fn listed_for_alpha(&self, of: &Fixture) -> Vec<String> {
        let listed = self
            .at(of, "meta/schedule/list", json!({ "identity": "alpha" }))
            .await;
        listed["result"]["schedules"]
            .as_array()
            .unwrap_or_else(|| panic!("a list of schedules: {listed:#?}"))
            .iter()
            .filter_map(|schedule| schedule["schedule_id"].as_str())
            .map(ToString::to_string)
            .collect()
    }

    /// The params text the host's schedule service stores for `schedule_id`.
    async fn stored_params(&self, schedule_id: &str) -> String {
        let stored = self
            .launch
            .service
            .get(&ScheduleId::parse(schedule_id).expect("a schedule id"))
            .await
            .expect("the schedule is stored");
        let meerkat::TargetBinding::HostRunnable(binding) = stored.target else {
            panic!("a console schedule targets the console's runnable");
        };
        binding.params.expect("a send has params").get().to_string()
    }

    async fn stop(self) {
        self.second.stop().await;
        self.launch.stop().await;
    }
}

/// A schedule of the second mob's `alpha` stores a send that names the
/// second mob, on a host that states its default mob: it is that agent's
/// and not the launch mob's agent's of that name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_a_second_mobs_schedule_names_that_mob() -> TestResult {
    let host = two_mob_host("console-schedules-door-stores", false).await?;
    let second_mob = host.second.runtime.handle().mob_id().clone();
    let of_second = host.stored_for_alpha_of_the_second_mob().await?;

    let send = stored_send(&host.launch, &of_second).await?;
    assert_eq!(
        send.mob_id.as_ref(),
        Some(&second_mob),
        "the send of the second mob's alpha names the second mob: {}",
        host.stored_params(&of_second).await
    );
    assert!(send.is_for(&host.default_mob, &second_mob, "alpha"));
    assert!(!send.is_for(&host.default_mob, host.default_mob.mob_id(), "alpha"));
    host.stop().await;
    Ok(())
}

/// Spawn the agent `alias` in `fixture`'s mob. With `takes_sends` false it
/// is an agent that takes no console send.
async fn spawn_agent(fixture: &Fixture, alias: &str, takes_sends: bool) -> TestResult {
    spawn_agent_of_role(fixture, "worker", alias, takes_sends).await
}

/// [`spawn_agent`] with the role `role` (`agent` in a fixture whose agents
/// take console sends).
async fn spawn_agent_of_role(
    fixture: &Fixture,
    role: &str,
    alias: &str,
    takes_sends: bool,
) -> TestResult {
    let mut spec =
        SpawnMemberSpec::from_wire(role.to_string(), alias.to_string(), None, None, None);
    if !takes_sends {
        spec = spec.with_labels(BTreeMap::from([(
            "addressable".to_string(),
            "false".to_string(),
        )]));
    }
    fixture.runtime.handle().spawn_spec(spec).await?;
    Ok(())
}

/// The fires reach the agents of the host's default mob alone, so the
/// schedule door refuses a schedule for the second mob's `alpha`, an agent
/// that mob's identity runtime knows, and says
/// that it would never be sent; nothing is stored. A schedule of that agent
/// that is in the store all the same is refused an update and a resume the
/// same way, and is listed, paused and deleted through its own mob. The
/// launch mob's `alpha` takes a schedule.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_refuses_a_schedule_no_fire_would_send() -> TestResult {
    let host =
        two_mob_host_with_an_identity_plane("console-schedules-door-refuses", &[("alpha", true)])
            .await?;
    let second_mob = host.second.runtime.handle().mob_id().clone();
    let never_sent = |answer: &Value, what: &str| {
        assert_eq!(
            error_of(answer),
            (
                json!(crate::rpc::CAPABILITY_UNAVAILABLE_CODE),
                json!("schedule_host_unavailable")
            ),
            "{what}: {answer:#?}"
        );
        let message = answer["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("would never be sent") && message.contains(second_mob.as_str()),
            "{what}: the refusal says that the schedule would never be sent, and names the \
             mob: {message}"
        );
    };

    let created = host
        .at(&host.second, "meta/schedule/create", create_params("alpha"))
        .await;
    never_sent(&created, "create");
    assert!(
        host.launch.service.list().await?.is_empty(),
        "a refused create stores nothing"
    );

    let of_second = host.stored_for_alpha_of_the_second_mob().await?;
    let stored_before = host.stored_params(&of_second).await;
    let by_id = json!({ "identity": "alpha", "schedule_id": of_second });
    let updated = host
        .at(
            &host.second,
            "meta/schedule/update",
            json!({ "identity": "alpha", "schedule_id": of_second, "content": "Changed." }),
        )
        .await;
    never_sent(&updated, "update");
    assert_eq!(host.stored_params(&of_second).await, stored_before);
    assert_eq!(
        host.listed_for_alpha(&host.second).await,
        std::slice::from_ref(&of_second),
        "the schedule is listed for its own agent"
    );
    let paused = host
        .at(&host.second, "meta/schedule/pause", by_id.clone())
        .await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    let resumed = host
        .at(&host.second, "meta/schedule/resume", by_id.clone())
        .await;
    never_sent(&resumed, "resume");
    assert_eq!(
        host.launch
            .service
            .get(&ScheduleId::parse(&of_second)?)
            .await?
            .phase,
        meerkat::SchedulePhase::Paused,
        "the refused resume changed nothing"
    );
    assert_eq!(host.stored_params(&of_second).await, stored_before);
    let deleted = host.at(&host.second, "meta/schedule/delete", by_id).await;
    assert!(deleted["error"].is_null(), "{deleted:#?}");

    let of_launch = host.created_for_alpha(&host.launch).await;
    assert_eq!(host.listed_for_alpha(&host.launch).await, [of_launch]);
    host.stop().await;
    Ok(())
}

/// A name is an agent's in the mob of the request only. Through the second
/// mob, a name that only an agent of the launch mob has is answered as a
/// name no agent has, whether that agent takes console sends or not, and a
/// list for it answers as for a name no agent has. Through the launch mob
/// the same names are its agents'.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_finds_no_agent_of_another_mob() -> TestResult {
    let host = two_mob_host("console-schedules-door-finds", false).await?;
    spawn_agent(&host.launch, "gamma", true).await?;
    spawn_agent(&host.launch, "quiet", false).await?;

    let of_no_agent = host
        .at(&host.second, "meta/schedule/create", create_params("ghost"))
        .await;
    assert_eq!(
        error_of(&of_no_agent),
        (json!(-32001), json!("unknown_identity")),
        "{of_no_agent:#?}"
    );
    for name in ["gamma", "quiet"] {
        let through_the_second_mob = host
            .at(&host.second, "meta/schedule/create", create_params(name))
            .await;
        assert_eq!(
            error_of(&through_the_second_mob),
            error_of(&of_no_agent),
            "{name} is no agent of the second mob: {through_the_second_mob:#?}"
        );
        let listed = host
            .at(
                &host.second,
                "meta/schedule/list",
                json!({ "identity": name }),
            )
            .await;
        assert_eq!(
            listed["result"],
            json!({ "identity": name, "schedules": [], "other_schedules": [] }),
            "{listed:#?}"
        );
    }
    assert!(
        host.launch.service.list().await?.is_empty(),
        "nothing was stored"
    );

    let of_gamma = host
        .at(&host.launch, "meta/schedule/create", create_params("gamma"))
        .await;
    assert!(of_gamma["error"].is_null(), "{of_gamma:#?}");
    let of_quiet = host
        .at(&host.launch, "meta/schedule/create", create_params("quiet"))
        .await;
    assert_eq!(
        error_of(&of_quiet),
        (json!(-32002), json!("not_addressable")),
        "{of_quiet:#?}"
    );
    host.stop().await;
    Ok(())
}

/// Two mobs' agents of one name are judged each in its own mob: whether
/// the agent takes console sends is its own, as its mob's identity runtime
/// says it, never its namesake's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_judges_an_agent_and_not_its_namesake() -> TestResult {
    // `echo` takes sends in the launch mob and none in the second; `mute`
    // the other way round.
    let host = two_mob_host_with_an_identity_plane(
        "console-schedules-door-judges",
        &[("alpha", true), ("echo", false), ("mute", true)],
    )
    .await?;
    spawn_agent(&host.launch, "echo", true).await?;
    spawn_agent(&host.launch, "mute", false).await?;
    let takes_no_sends = (json!(-32002), json!("not_addressable"));

    let echo_of_the_second_mob = host
        .at(&host.second, "meta/schedule/create", create_params("echo"))
        .await;
    assert_eq!(
        error_of(&echo_of_the_second_mob),
        takes_no_sends,
        "the second mob's echo takes no sends: {echo_of_the_second_mob:#?}"
    );
    let mute_of_the_second_mob = host
        .at(&host.second, "meta/schedule/create", create_params("mute"))
        .await;
    let alpha_of_the_second_mob = host
        .at(&host.second, "meta/schedule/create", create_params("alpha"))
        .await;
    assert_eq!(
        error_of(&mute_of_the_second_mob),
        error_of(&alpha_of_the_second_mob),
        "the second mob's mute takes sends, as its alpha does: {mute_of_the_second_mob:#?}"
    );
    assert_ne!(error_of(&mute_of_the_second_mob), takes_no_sends);

    let echo_of_the_launch_mob = host
        .at(&host.launch, "meta/schedule/create", create_params("echo"))
        .await;
    assert!(
        echo_of_the_launch_mob["error"].is_null(),
        "the launch mob's echo takes sends: {echo_of_the_launch_mob:#?}"
    );
    let mute_of_the_launch_mob = host
        .at(&host.launch, "meta/schedule/create", create_params("mute"))
        .await;
    assert_eq!(
        error_of(&mute_of_the_launch_mob),
        takes_no_sends,
        "the launch mob's mute takes no sends: {mute_of_the_launch_mob:#?}"
    );
    host.stop().await;
    Ok(())
}

/// An agent the second mob's identity runtime knows, with no namesake in
/// the launch mob, is an agent to a call for the second mob: the door
/// answers for it as for the second mob's `alpha`, and not as for a name no
/// agent has. Through the launch mob that name is no agent's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_knows_an_agent_of_the_requests_mob_without_a_namesake()
-> TestResult {
    let host = two_mob_host_with_an_identity_plane(
        "console-schedules-door-knows",
        &[("alpha", true), ("delta", true)],
    )
    .await?;
    let no_agent = (json!(-32001), json!("unknown_identity"));

    let delta = host
        .at(&host.second, "meta/schedule/create", create_params("delta"))
        .await;
    let alpha = host
        .at(&host.second, "meta/schedule/create", create_params("alpha"))
        .await;
    assert_ne!(
        error_of(&delta),
        no_agent,
        "the second mob's delta is an agent of it: {delta:#?}"
    );
    assert_eq!(
        error_of(&delta),
        error_of(&alpha),
        "the second mob's delta is answered as its alpha is: {delta:#?}"
    );

    let through_the_launch_mob = host
        .at(&host.launch, "meta/schedule/create", create_params("delta"))
        .await;
    assert_eq!(
        error_of(&through_the_launch_mob),
        no_agent,
        "delta is no agent of the launch mob: {through_the_launch_mob:#?}"
    );
    host.stop().await;
    Ok(())
}

/// For a mob other than the host's default mob the door knows the agents
/// that mob's identity runtime knows, and no other name. A member of the
/// second mob that its identity runtime does not know is answered as a name
/// no agent has, and so is every member of a second mob that has no
/// identity runtime; a list for such a name answers as for a name no agent
/// has. The agent the identity runtime knows is an agent, and the launch
/// mob's own member is found by the console's records.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_knows_another_mobs_agents_by_its_identity_runtime_alone()
-> TestResult {
    let no_agent = (json!(-32001), json!("unknown_identity"));

    // A second mob whose identity runtime knows `alpha`, with a member
    // `delta` that it does not know.
    let host =
        two_mob_host_with_an_identity_plane("console-schedules-door-plane", &[("alpha", true)])
            .await?;
    spawn_agent_of_role(&host.second, "agent", "delta", true).await?;
    let of_no_agent = host
        .at(&host.second, "meta/schedule/create", create_params("ghost"))
        .await;
    assert_eq!(error_of(&of_no_agent), no_agent, "{of_no_agent:#?}");
    let of_the_member = host
        .at(&host.second, "meta/schedule/create", create_params("delta"))
        .await;
    assert_eq!(
        error_of(&of_the_member),
        no_agent,
        "the second mob's identity runtime does not know delta: {of_the_member:#?}"
    );
    let listed = host
        .at(
            &host.second,
            "meta/schedule/list",
            json!({ "identity": "delta" }),
        )
        .await;
    assert_eq!(
        listed["result"],
        json!({ "identity": "delta", "schedules": [], "other_schedules": [] }),
        "{listed:#?}"
    );
    let of_the_agent = host
        .at(&host.second, "meta/schedule/create", create_params("alpha"))
        .await;
    assert_ne!(
        error_of(&of_the_agent),
        no_agent,
        "the second mob's identity runtime knows alpha: {of_the_agent:#?}"
    );
    assert!(
        host.launch.service.list().await?.is_empty(),
        "nothing was stored"
    );
    host.stop().await;

    // A second mob with no identity runtime: none of its members is an
    // agent to the door, and the launch mob's member still is.
    let host = two_mob_host("console-schedules-door-no-plane", false).await?;
    let of_the_member = host
        .at(&host.second, "meta/schedule/create", create_params("alpha"))
        .await;
    assert_eq!(
        error_of(&of_the_member),
        no_agent,
        "a second mob without an identity runtime: {of_the_member:#?}"
    );
    let of_the_launch_mobs_member = host
        .at(&host.launch, "meta/schedule/create", create_params("alpha"))
        .await;
    assert!(
        of_the_launch_mobs_member["error"].is_null(),
        "{of_the_launch_mobs_member:#?}"
    );
    host.stop().await;
    Ok(())
}

/// On a gateway that serves one mob the door reads the console's records
/// as its send resolves them: an agent of the mob is found by its name and
/// by its runtime member id, a name no agent has is refused, and a
/// schedule stores the bytes every released build stores.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_of_a_one_mob_gateway_reads_the_consoles_records() -> TestResult
{
    let fixture = fixture(
        "console-schedules-door-one-mob",
        Some(ScheduleFiringHostBinding::external_driver()),
    )
    .await?;
    let default_mob = crate::member_key::DefaultMob::new(fixture.runtime.handle().mob_id().clone());
    let records = fixture.aggregator.identity_records_now().await?;
    let alpha = records
        .iter()
        .find(|record| record.identity == "alpha")
        .ok_or("the console's record of alpha")?;
    let at = |method: &'static str, params: Value| {
        at_the_schedule_door(&default_mob, &fixture, &fixture.aggregator, method, params)
    };

    let created = at("meta/schedule/create", create_params("alpha")).await;
    let schedule_id = answered_id(&created);
    let defined_at = stored_send(&fixture, &schedule_id).await?.defined_at;
    let stored = fixture
        .service
        .get(&ScheduleId::parse(&schedule_id)?)
        .await?;
    let meerkat::TargetBinding::HostRunnable(binding) = stored.target else {
        return Err("a console schedule targets the console's runnable".into());
    };
    assert_eq!(
        binding.params.ok_or("a send has params")?.get(),
        format!(
            r#"{{"content":"Post the morning summary.","defined_at":{defined_at},"identity":"alpha","schema":"meta.console.scheduled_send.v1","sender":{{"kind":"anonymous"}}}}"#
        )
    );
    let by_member_id = at(
        "meta/schedule/list",
        json!({ "identity": alpha.runtime_member_id }),
    )
    .await;
    assert_eq!(
        by_member_id["result"]["identity"],
        json!("alpha"),
        "{by_member_id:#?}"
    );
    assert_eq!(
        by_member_id["result"]["schedules"][0]["schedule_id"],
        json!(schedule_id),
        "{by_member_id:#?}"
    );
    let of_no_agent = at("meta/schedule/create", create_params("ghost")).await;
    assert_eq!(
        error_of(&of_no_agent),
        (json!(-32001), json!("unknown_identity")),
        "{of_no_agent:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A schedule of the second mob's `alpha` is listed for that agent and not
/// for the launch mob's agent of that name, and the launch mob's own
/// schedule is listed for its own `alpha` only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_lists_a_second_mobs_schedule_for_its_own_agent() -> TestResult {
    let host = two_mob_host("console-schedules-door-lists", false).await?;
    let of_second = host.stored_for_alpha_of_the_second_mob().await?;
    let of_launch = host.created_for_alpha(&host.launch).await;

    assert_eq!(
        host.listed_for_alpha(&host.launch).await,
        [of_launch],
        "the launch mob's alpha lists its own schedule only"
    );
    assert_eq!(
        host.listed_for_alpha(&host.second).await,
        [of_second],
        "the second mob's alpha lists its own schedule only"
    );
    host.stop().await;
    Ok(())
}

/// A schedule that is no Console schedule and names an agent by its name
/// alone is the default mob's agent's: the door lists it on that agent's
/// view, and not on the view of the agent of the same name in another mob
/// of the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_lists_a_schedule_that_names_no_mob_for_the_default_mobs_agent()
-> TestResult {
    let host = two_mob_host("console-schedules-door-no-mob", false).await?;
    let schedule = host
        .launch
        .service
        .create(meerkat::CreateScheduleRequest {
            name: Some("a note".to_owned()),
            description: None,
            trigger: meerkat::TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: meerkat::TargetBinding::identity(meerkat::IdentityTargetBinding::resumable(
                "alpha",
                meerkat::ScheduledSessionAction::Prompt {
                    prompt: meerkat_core::ContentInput::Text("Say hello.".into()),
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
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await
        .map_err(|error| error.to_string())?
        .schedule_id
        .to_string();

    let through_the_launch_mob = host
        .at(
            &host.launch,
            "meta/schedule/list",
            json!({ "identity": "alpha" }),
        )
        .await;
    assert_eq!(
        through_the_launch_mob["result"]["other_schedules"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|other| other["schedule_id"].clone())
            .collect::<Vec<_>>(),
        vec![json!(schedule)],
        "the default mob's agent's view lists the schedule that names it by its name alone: \
         {through_the_launch_mob:#?}"
    );
    let through_the_second_mob = host
        .at(
            &host.second,
            "meta/schedule/list",
            json!({ "identity": "alpha" }),
        )
        .await;
    assert_eq!(
        through_the_second_mob["result"]["other_schedules"],
        json!([]),
        "the view of the second mob's agent of that name does not list it: \
         {through_the_second_mob:#?}"
    );
    host.stop().await;
    Ok(())
}

/// A schedule of the second mob's `alpha` is not changed through the launch
/// mob's agent of that name: an update, a pause, a resume and a delete that
/// name it are answered as those of another agent's schedule are, and change
/// nothing. Through its own mob it pauses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_changes_a_second_mobs_schedule_through_its_own_agent()
-> TestResult {
    let host = two_mob_host("console-schedules-door-changes", false).await?;
    let of_second = host.stored_for_alpha_of_the_second_mob().await?;
    let of_launch = host.created_for_alpha(&host.launch).await;
    let stored_before = host.stored_params(&of_second).await;

    for method in [
        "meta/schedule/update",
        "meta/schedule/pause",
        "meta/schedule/resume",
        "meta/schedule/delete",
    ] {
        let through_the_launch_mob = host
            .at(
                &host.launch,
                method,
                json!({ "identity": "alpha", "schedule_id": of_second }),
            )
            .await;
        let of_another_agent = host
            .at(
                &host.launch,
                method,
                json!({ "identity": "beta", "schedule_id": of_launch }),
            )
            .await;
        assert!(
            !through_the_launch_mob["error"].is_null(),
            "{method}: {through_the_launch_mob:#?}"
        );
        assert_eq!(
            error_of(&through_the_launch_mob),
            error_of(&of_another_agent),
            "{method}: a schedule of the agent of that name in another mob is another agent's"
        );
    }
    let unchanged = host
        .launch
        .service
        .get(&ScheduleId::parse(&of_second)?)
        .await?;
    assert_eq!(unchanged.phase, meerkat::SchedulePhase::Active);
    assert_eq!(host.stored_params(&of_second).await, stored_before);

    let paused = host
        .at(
            &host.second,
            "meta/schedule/pause",
            json!({ "identity": "alpha", "schedule_id": of_second }),
        )
        .await;
    assert!(paused["error"].is_null(), "{paused:#?}");
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    host.stop().await;
    Ok(())
}

/// A schedule of the second mob's `alpha` is not fired at the launch mob's
/// agent of that name: the launch mob's send path refuses the fire and sends
/// nothing, while the launch mob's own schedule fires and is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_a_second_mobs_schedule_fires_at_no_launch_mob_agent()
-> TestResult {
    let host = two_mob_host("console-schedules-door-fires", true).await?;
    let of_second = host.stored_for_alpha_of_the_second_mob().await?;
    let of_launch = host.created_for_alpha(&host.launch).await;

    let open = doors(
        &host.launch,
        open_decisions(false),
        None,
        crate::auth::session_fence::ConsoleSessionFences::detached(),
        ungoverned(),
    );
    refused_because(
        fire(&host.launch, &open, &of_second, "key-of-the-second-mob").await,
        "a member of mob",
    );
    assert!(
        sends_under(&host.launch, "alpha", "key-of-the-second-mob")
            .await
            .is_empty(),
        "nothing was sent to the launch mob's agent of the same name"
    );
    fire(&host.launch, &open, &of_launch, "key-of-the-launch-mob").await?;
    assert_eq!(
        sends_under(&host.launch, "alpha", "key-of-the-launch-mob")
            .await
            .len(),
        1
    );
    host.stop().await;
    Ok(())
}

/// A schedule created for the launch mob's agent stores the bytes every
/// released build stores: the first schema string and no mob. Passes before
/// and after the door is told the host's default mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_a_launch_mobs_schedule_stores_the_released_bytes() -> TestResult
{
    let host = two_mob_host("console-schedules-door-bytes", false).await?;
    let of_launch = host.created_for_alpha(&host.launch).await;
    let defined_at = stored_send(&host.launch, &of_launch).await?.defined_at;
    assert_eq!(
        host.stored_params(&of_launch).await,
        format!(
            r#"{{"content":"Post the morning summary.","defined_at":{defined_at},"identity":"alpha","schema":"meta.console.scheduled_send.v1","sender":{{"kind":"anonymous"}}}}"#
        )
    );
    host.stop().await;
    Ok(())
}

/// `method` with `params` posted to the request door of the open console
/// `router`: the JSON-RPC answer.
async fn post_to_the_open_console(router: &axum::Router, method: &str, params: Value) -> Value {
    use tower::ServiceExt;
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
                        .to_string(),
                ))
                .expect("console request"),
        )
        .await
        .expect("console response");
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("console body");
    serde_json::from_slice(&body).unwrap_or(Value::Null)
}

/// End to end, through the console's request door, for all six methods: on
/// a host whose resolver serves a second mob (a test-only seam; no host can
/// serve one yet) whose identity runtime knows `alpha`, a
/// `meta/schedule/create` that names the second mob is refused, because no
/// fire would send the schedule, and stores nothing; a name that only an
/// agent of the launch mob has is answered as a name no agent has. A
/// schedule of the second mob's `alpha` that is in the store is listed for
/// that agent and not for the launch mob's agent of that name, and a pause
/// that names it through the launch mob answers as for another agent's
/// schedule. Named for the second mob, an update and a resume of it are
/// refused as the create is, and a pause and a delete answer. A create that
/// names no mob stores the bytes every released build stores, and that
/// schedule stays the launch mob's agent's throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_through_the_console_a_second_mobs_schedule_is_its_own()
-> TestResult {
    let launch = delivering_fixture("console-schedules-rpc-launch").await?;
    let second = identity_plane_fixture("console-schedules-rpc-second", &[("alpha", true)]).await?;
    // One host: both mobs' console schedules live in its one schedule
    // service, the launch's.
    second.runtime.console_schedules().attach(
        launch.service.clone(),
        ScheduleFiringHostBinding::external_driver(),
    );
    let second_mob = second.runtime.handle().mob_id().clone();
    let resolver = launch
        .owner
        .as_ref()
        .ok_or("the launch's runtime")?
        .mob_resolver()
        .serving_a_second_mob_in_tests(
            &second
                .owner
                .as_ref()
                .ok_or("the second mob's runtime")?
                .mob_resolver(),
        );
    let mut state = super::console_private_side_chat_tests::open_console(&launch.runtime);
    state.console_aggregator = Some(launch.aggregator.clone());
    state.console_events = Some(launch.events.clone());
    let router = console_json_router_with_state(state).layer(axum::Extension(resolver));
    let naming = |mut params: Value, mob: Option<&meerkat_mob::MobId>| {
        if let Some(mob) = mob {
            params["mob_id"] = json!(mob.as_str());
        }
        params
    };
    let listed = |answer: &Value| -> Vec<String> {
        answer["result"]["schedules"]
            .as_array()
            .unwrap_or_else(|| panic!("a list of schedules: {answer:#?}"))
            .iter()
            .filter_map(|schedule| schedule["schedule_id"].as_str())
            .map(ToString::to_string)
            .collect()
    };

    let refused = post_to_the_open_console(
        &router,
        "meta/schedule/create",
        naming(create_params("alpha"), Some(&second_mob)),
    )
    .await;
    assert_eq!(
        error_of(&refused),
        (
            json!(crate::rpc::CAPABILITY_UNAVAILABLE_CODE),
            json!("schedule_host_unavailable")
        ),
        "no fire would send a schedule of the second mob's alpha: {refused:#?}"
    );
    spawn_agent_of_role(&launch, "agent", "gamma", true).await?;
    let of_no_agent = post_to_the_open_console(
        &router,
        "meta/schedule/create",
        naming(create_params("ghost"), Some(&second_mob)),
    )
    .await;
    assert_eq!(
        error_of(&of_no_agent),
        (json!(-32001), json!("unknown_identity")),
        "{of_no_agent:#?}"
    );
    let of_the_launch_mobs_agent = post_to_the_open_console(
        &router,
        "meta/schedule/create",
        naming(create_params("gamma"), Some(&second_mob)),
    )
    .await;
    assert_eq!(
        error_of(&of_the_launch_mobs_agent),
        error_of(&of_no_agent),
        "gamma is no agent of the second mob: {of_the_launch_mobs_agent:#?}"
    );
    assert!(
        launch.service.list().await?.is_empty(),
        "the refused creates stored nothing"
    );
    let default_mob = crate::member_key::DefaultMob::new(launch.runtime.handle().mob_id().clone());
    let of_second = stored_for_alpha_of(&launch.service, &default_mob, &second_mob).await?;
    let send = stored_send(&launch, &of_second).await?;
    assert_eq!(send.mob_id.as_ref(), Some(&second_mob));
    assert_eq!(send.schema, "meta.console.scheduled_send.v2");

    let of_launch = answered_id(
        &post_to_the_open_console(&router, "meta/schedule/create", create_params("alpha")).await,
    );
    let send = stored_send(&launch, &of_launch).await?;
    assert_eq!(send.mob_id, None, "a request that names no mob");
    assert_eq!(send.schema, "meta.console.scheduled_send.v1");

    let for_alpha = json!({ "identity": "alpha" });
    assert_eq!(
        listed(
            &post_to_the_open_console(
                &router,
                "meta/schedule/list",
                naming(for_alpha.clone(), Some(&second_mob)),
            )
            .await
        ),
        std::slice::from_ref(&of_second),
        "the second mob's alpha lists its own schedule only"
    );
    assert_eq!(
        listed(&post_to_the_open_console(&router, "meta/schedule/list", for_alpha).await),
        std::slice::from_ref(&of_launch),
        "the launch mob's alpha lists its own schedule only"
    );

    let through_the_launch_mob = post_to_the_open_console(
        &router,
        "meta/schedule/pause",
        json!({ "identity": "alpha", "schedule_id": of_second }),
    )
    .await;
    let of_another_agent = post_to_the_open_console(
        &router,
        "meta/schedule/pause",
        json!({ "identity": "beta", "schedule_id": of_launch }),
    )
    .await;
    assert!(
        !through_the_launch_mob["error"].is_null(),
        "{through_the_launch_mob:#?}"
    );
    assert_eq!(
        error_of(&through_the_launch_mob),
        error_of(&of_another_agent),
        "a schedule of the agent of that name in another mob is another agent's"
    );

    // The other writes, each named for the second mob: an update and a
    // resume are refused as the create is, a pause and a delete answer, and
    // the launch mob's schedule is not touched.
    let never_sent = (
        json!(crate::rpc::CAPABILITY_UNAVAILABLE_CODE),
        json!("schedule_host_unavailable"),
    );
    let by_id = naming(
        json!({ "identity": "alpha", "schedule_id": of_second }),
        Some(&second_mob),
    );
    let updated = post_to_the_open_console(
        &router,
        "meta/schedule/update",
        naming(
            json!({ "identity": "alpha", "schedule_id": of_second, "content": "Changed." }),
            Some(&second_mob),
        ),
    )
    .await;
    assert_eq!(error_of(&updated), never_sent, "{updated:#?}");
    let paused = post_to_the_open_console(&router, "meta/schedule/pause", by_id.clone()).await;
    assert_eq!(paused["result"]["phase"], json!("paused"), "{paused:#?}");
    let resumed = post_to_the_open_console(&router, "meta/schedule/resume", by_id.clone()).await;
    assert_eq!(error_of(&resumed), never_sent, "{resumed:#?}");
    assert_eq!(
        launch
            .service
            .get(&ScheduleId::parse(&of_second)?)
            .await?
            .phase,
        meerkat::SchedulePhase::Paused,
        "the refused resume changed nothing"
    );
    let deleted = post_to_the_open_console(&router, "meta/schedule/delete", by_id).await;
    assert!(deleted["error"].is_null(), "{deleted:#?}");
    assert!(
        listed(
            &post_to_the_open_console(
                &router,
                "meta/schedule/list",
                naming(json!({ "identity": "alpha" }), Some(&second_mob)),
            )
            .await
        )
        .is_empty(),
        "the second mob's alpha has no schedule left"
    );
    assert_eq!(
        listed(
            &post_to_the_open_console(
                &router,
                "meta/schedule/list",
                json!({ "identity": "alpha" })
            )
            .await
        ),
        std::slice::from_ref(&of_launch),
        "the launch mob's alpha keeps its schedule"
    );
    assert_eq!(
        launch
            .service
            .get(&ScheduleId::parse(&of_launch)?)
            .await?
            .phase,
        meerkat::SchedulePhase::Active
    );
    second.stop().await;
    launch.stop().await;
    Ok(())
}

/// End to end, for signed-in callers under an access policy whose rules
/// select mobs, on a host whose resolver serves a second mob (a test-only
/// seam). A person whose allow selects the second mob reaches the schedule
/// door for that mob: a schedule for its `alpha` is refused because no fire
/// would send it, a name that only an agent of the launch mob has is
/// answered as a name no agent has, and nothing is stored; the same person
/// has no rule in the launch mob. A person whose allow selects the launch
/// mob is answered for the second mob as for an id no mob has, and
/// schedules the launch mob's `alpha` with a send that names no mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_schedule_door_through_the_console_for_signed_in_callers() -> TestResult {
    use super::console_human_admission_tests::BOB;
    use super::console_private_side_chat_tests::{
        ALICE, ROOT, alice_claims, bearer, bob_claims, enforced_console,
    };
    use super::console_token_lapse_tests::console_rpc_as;
    let launch = delivering_fixture("console-schedules-signed-launch").await?;
    let second =
        identity_plane_fixture("console-schedules-signed-second", &[("alpha", true)]).await?;
    second.runtime.console_schedules().attach(
        launch.service.clone(),
        ScheduleFiringHostBinding::external_driver(),
    );
    spawn_agent_of_role(&launch, "agent", "gamma", true).await?;
    let second_mob = second.runtime.handle().mob_id().clone();
    let schedules_agents = [ACTION_AGENT_VIEW, ACTION_AGENT_SEND, "schedule.manage"]
        .map(str::to_string)
        .to_vec();
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                id: "alice-schedules-the-second-mob".to_string(),
                subjects: vec![ALICE.to_string()],
                actions: schedules_agents.clone(),
                agents: vec!["*".to_string()],
                mobs: vec![second_mob.to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "bob-schedules-the-launch-mob".to_string(),
                subjects: vec![BOB.to_string()],
                actions: schedules_agents,
                agents: vec!["*".to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })?;
    let mut state = enforced_console(&launch.runtime, Some(access));
    state.console_aggregator = Some(launch.aggregator.clone());
    state.console_events = Some(launch.events.clone());
    let resolver = launch
        .owner
        .as_ref()
        .ok_or("the launch's runtime")?
        .mob_resolver()
        .serving_a_second_mob_in_tests(
            &second
                .owner
                .as_ref()
                .ok_or("the second mob's runtime")?
                .mob_resolver(),
        );
    let router = console_json_router_with_state(state).layer(axum::Extension(resolver));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let naming = |mut params: Value, mob: &str| {
        params["mob_id"] = json!(mob);
        params
    };
    let no_agent = (json!(-32001), json!("unknown_identity"));

    // A person whose rules select the launch mob alone.
    let no_mob_has = format!("{second_mob}-absent");
    for (method, params) in [
        ("meta/schedule/list", json!({ "identity": "alpha" })),
        ("meta/schedule/create", create_params("alpha")),
    ] {
        let of_the_second_mob = console_rpc_as(
            &router,
            Some(&bob),
            method,
            naming(params.clone(), second_mob.as_str()),
        )
        .await;
        let of_no_mob =
            console_rpc_as(&router, Some(&bob), method, naming(params, &no_mob_has)).await;
        assert_eq!(
            error_of(&of_the_second_mob),
            (json!(-32602), json!("foreign_mob_target")),
            "{method}: {of_the_second_mob:#?}"
        );
        assert_eq!(
            error_of(&of_the_second_mob),
            error_of(&of_no_mob),
            "{method}: a mob the caller may not address is answered as an id no mob has"
        );
    }

    // A person whose rule selects the second mob.
    let refused = console_rpc_as(
        &router,
        Some(&alice),
        "meta/schedule/create",
        naming(create_params("alpha"), second_mob.as_str()),
    )
    .await;
    assert_eq!(
        error_of(&refused),
        (
            json!(crate::rpc::CAPABILITY_UNAVAILABLE_CODE),
            json!("schedule_host_unavailable")
        ),
        "no fire would send a schedule of the second mob's alpha: {refused:#?}"
    );
    let of_no_agent = console_rpc_as(
        &router,
        Some(&alice),
        "meta/schedule/create",
        naming(create_params("ghost"), second_mob.as_str()),
    )
    .await;
    assert_eq!(error_of(&of_no_agent), no_agent, "{of_no_agent:#?}");
    let of_the_launch_mobs_agent = console_rpc_as(
        &router,
        Some(&alice),
        "meta/schedule/create",
        naming(create_params("gamma"), second_mob.as_str()),
    )
    .await;
    assert_eq!(
        error_of(&of_the_launch_mobs_agent),
        no_agent,
        "gamma is no agent of the second mob: {of_the_launch_mobs_agent:#?}"
    );
    let listed = console_rpc_as(
        &router,
        Some(&alice),
        "meta/schedule/list",
        naming(json!({ "identity": "alpha" }), second_mob.as_str()),
    )
    .await;
    assert_eq!(
        listed["result"],
        json!({ "identity": "alpha", "schedules": [], "other_schedules": [] }),
        "{listed:#?}"
    );
    let in_the_launch_mob = console_rpc_as(
        &router,
        Some(&alice),
        "meta/schedule/create",
        create_params("alpha"),
    )
    .await;
    assert_eq!(
        error_of(&in_the_launch_mob),
        (json!(ACCESS_DENIED_RPC_CODE), json!("access_denied")),
        "a rule that selects the second mob gives nothing in the launch mob: \
         {in_the_launch_mob:#?}"
    );
    assert!(
        launch.service.list().await?.is_empty(),
        "nothing was stored"
    );

    let of_launch = answered_id(
        &console_rpc_as(
            &router,
            Some(&bob),
            "meta/schedule/create",
            create_params("alpha"),
        )
        .await,
    );
    let send = stored_send(&launch, &of_launch).await?;
    assert_eq!(send.mob_id, None, "a request that names no mob");
    assert_eq!(send.schema, "meta.console.scheduled_send.v1");
    second.stop().await;
    launch.stop().await;
    Ok(())
}
