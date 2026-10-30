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
    firing: Option<ScheduleFiringHostBinding>,
) -> Fixture {
    let aggregator = MetaConsoleAggregator::in_memory();
    let events = ConsoleEventStore::new();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
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
    Ok(fixture_over(temp_dir, None, runtime, firing))
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
        Some(ScheduleFiringHostBinding::external_driver()),
    ))
}

/// Who calls: a verified Console human (its principal, with an OIDC subject
/// of its own) or the open Console's shared account, on a writable or
/// read-only console, under an access policy or none.
#[derive(Clone, Copy)]
struct Caller<'a> {
    principal: Option<&'a str>,
    read_only: bool,
    access: Option<&'a AccessController>,
}

impl<'a> Caller<'a> {
    fn human(principal: &'a str) -> Self {
        Self {
            principal: Some(principal),
            read_only: false,
            access: None,
        }
    }

    fn open_console() -> Self {
        Self {
            principal: None,
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
        None,
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

/// The list is a read of the agent and every write a send to it; the
/// writes are console mutations a read-only console and a candidate launch
/// refuse.
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
            Some(vec![(ACTION_AGENT_SEND, Some("alpha".to_string()))]),
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
/// listed under that agent only, changed into the changer's send, paused,
/// resumed as the resumer's send, and deleted.
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

    // A change is the changer's send; what it does not name stays.
    let updated = call(
        &fixture,
        Caller::human(FRANK),
        "meta/schedule/update",
        json!({
            "identity": "alpha",
            "schedule_id": schedule_id,
            "content": "Post the evening summary.",
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

    // A resume is the resumer's send.
    let resumed = call(
        &fixture,
        Caller::human(ERIN),
        "meta/schedule/resume",
        json!({ "identity": "alpha", "schedule_id": schedule_id }),
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
/// `decisions` and `access`, checking sessions against `session_fences`.
fn doors(
    fixture: &Fixture,
    decisions: crate::runtime::RuntimeDecisionState,
    access: Option<AccessController>,
    session_fences: crate::auth::session_fence::ConsoleSessionFences,
) -> ConsoleScheduleDoorSlot {
    let doors = ConsoleScheduleDoorSlot::default();
    doors.install(Arc::new(schedules::ConsoleSendDoor {
        decisions,
        runtime: fixture.runtime.clone(),
        aggregator: fixture.aggregator.clone(),
        access,
        identity_runtime: None,
        console_events: fixture.events.clone(),
        governed_ingress: crate::console_governed_ingress::ConsoleGovernedIngressSlot::default(),
        operator_resolver: None,
        session_fences,
    }));
    doors
}

fn open_decisions(read_only: bool) -> crate::runtime::RuntimeDecisionState {
    crate::runtime::RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            read_only,
            ..crate::ConsolePolicy::default()
        },
        None,
    )
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

    // Judged at the fire: a sender who lost `agent.send` sends nothing.
    let viewer_only = enforced(vec![grant(
        "erin-views-alpha",
        ERIN,
        &[ACTION_AGENT_VIEW],
        "alpha",
    )])?;
    let revoked_grant = doors(
        &fixture,
        open_decisions(false),
        Some(viewer_only),
        fences.clone(),
    );
    let refused_key = format!("schedule:{schedule_id}:occurrence:2");
    let detail = match fire(&fixture, &revoked_grant, &schedule_id, &refused_key).await {
        Err(meerkat::HostRunnableError::Failed { detail }) => detail,
        other => panic!("a sender without agent.send is refused: {other:?}"),
    };
    assert!(detail.contains("not allowed to send"), "{detail}");
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
