//! What the console decides of an agent it holds no roster row of: a member
//! that has ended, a member seated in a mob the console does not list, an
//! agent its access records do not know. The console knows such an agent's
//! name and who it was private to; its role, its other labels and who
//! spawned it are not known, and a rule on any of them is decided closed: a
//! deny applies, an allow grants nothing. The doors of the console that read
//! or act on one member answer for an ended member as they answered while
//! it lived, with the gateway that seated it still running and after a
//! restart.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_client::TestClient;
use meerkat_core::types::SessionId;
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::{Value, json};

use super::*;
use crate::access::AccessControlConfig;
use crate::console_aggregator::AllowAllConsoleVisibilityPolicy;
use crate::{MobBootstrapOptions, MobBootstrapSpec};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The profiles every mob of these rows offers: a worker, and `delegate`,
/// the profile the helpers of the `delegate` tool run under.
fn profiles() -> Value {
    json!({
        "worker": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true }
        },
        "delegate": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true }
        }
    })
}

/// A runtime whose mob is `mob_id`, in memory, with agent mob tools.
async fn runtime(mob_id: &str, root: &std::path::Path) -> TestResult<MobRuntime> {
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": mob_id, "profiles": profiles() }))?;
    let runtime = MobRuntime::bootstrap(
        MobBootstrapSpec::ephemeral(
            definition,
            MobStorage::in_memory(),
            root.to_path_buf(),
            16,
            None,
        )
        .expect("build the fixture spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        }),
    )
    .await?;
    Ok(runtime)
}

/// Seat `member` on `handle` under `profile`, turn-driven, so no model runs.
async fn seat(
    handle: &MobHandle,
    profile: &str,
    member: &str,
    labels: BTreeMap<String, String>,
) -> TestResult {
    let mut spec = SpawnMemberSpec::new(ProfileName::from(profile), AgentIdentity::from(member));
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    if !labels.is_empty() {
        spec.labels = Some(labels);
    }
    handle.ensure_member(spec).await?;
    Ok(())
}

/// A mob made through the runtime's agent mob state beside the runtime's
/// own, owned by no member.
async fn mob_beside(runtime: &MobRuntime, mob_id: &str) -> TestResult<MobHandle> {
    let state = runtime
        .agent_mob_mcp_state()
        .ok_or("the runtime carries agent mob tools")?;
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": mob_id, "profiles": profiles() }))?;
    let created = Box::pin(state.mob_create_definition(definition))
        .await
        .map_err(|error| error.to_string())?;
    Ok(Box::pin(state.handle_for(&created))
        .await
        .map_err(|error| error.to_string())?)
}

/// The answer of the console RPC `method` with `params` for the holder of
/// `bearer`.
async fn rpc_answer(app: &Router, bearer: &str, method: &str, params: Value) -> Value {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::from(
                    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
                        .to_string(),
                ))
                .expect("rpc request"),
        )
        .await
        .expect("rpc response");
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).unwrap_or(Value::Null)
}

/// The enforced configuration of the rows on ended members: Alice may do
/// everything to every agent, then `more`.
fn alice_drives_every_agent_and(more: Vec<crate::access::AccessRule>) -> AccessControlConfig {
    let mut rules = vec![crate::access::AccessRule {
        agents: vec!["*".to_string()],
        ..agent_rule("alice-drives-every-agent", "alice@example.test")
    }];
    rules.extend(more);
    AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules,
        ..AccessControlConfig::default()
    }
}

/// An allow rule over every agent action for `subject`, with no selector.
fn agent_rule(id: &str, subject: &str) -> crate::access::AccessRule {
    crate::access::AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: vec!["agent.*".to_string()],
        ..crate::access::AccessRule::default()
    }
}

/// `agent_rule` as a deny.
fn agent_deny(id: &str, subject: &str) -> crate::access::AccessRule {
    crate::access::AccessRule {
        effect: crate::access::AccessEffect::Deny,
        ..agent_rule(id, subject)
    }
}

/// Retire `member` of `mob` and wait until it left the roster.
async fn retired(mob: &MobHandle, member: &str) -> TestResult {
    let identity = AgentIdentity::from(member);
    mob.retire(identity.clone()).await?;
    for _ in 0..100 {
        if mob.get_member(&identity).await?.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if mob.get_member(&identity).await?.is_some() {
        return Err(format!("{member} did not leave its roster").into());
    }
    Ok(())
}

/// A console schedule for `member`, stored in `service` as the console
/// stores one a person defined: its text is `scheduled-for-<member>`. The
/// schedule's id.
async fn schedule_for(service: &meerkat::ScheduleService, member: &str) -> TestResult<String> {
    let send = crate::console_schedules::ConsoleScheduledSend::new(
        member.to_string(),
        format!("scheduled-for-{member}"),
        crate::console_schedules::ConsoleScheduleSender::Human {
            principal: "alice@example.test".to_string(),
            provider: "google_oauth".to_string(),
            issuer: "https://issuer.example.test".to_string(),
            subject: "oidc-sub-alice".to_string(),
        },
        1,
    );
    let schedule = service
        .create(meerkat::CreateScheduleRequest {
            name: None,
            description: None,
            trigger: serde_json::from_value(json!({
                "type": "interval",
                "start_at_utc": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                "every_seconds": 3600,
            }))?,
            target: send.target()?,
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await?;
    Ok(schedule.schedule_id.to_string())
}

/// What `method` answers the holder of `bearer` for the schedule
/// `schedule_id` of `member`, with the member's name written as `MEMBER`.
async fn schedule_answer(
    app: &Router,
    bearer: &str,
    method: &str,
    member: &str,
    schedule_id: &str,
) -> String {
    let mut params = json!({ "identity": member });
    if method != "meta/schedule/list" {
        params["schedule_id"] = json!(schedule_id);
    }
    let answer = rpc_answer(app, bearer, method, params).await;
    json!({ "result": answer["result"], "error": answer["error"] })
        .to_string()
        .replace(member, "MEMBER")
}

/// What is wrong with how the console decides the ended `member` for Bob,
/// whom a rule of `config` refuses it, and for Alice, whom none does: on a
/// controller primed as every route primes one, Bob has neither `agent.view`
/// nor `agent.send` on it, the decision fence shows him no request it
/// raised, an administrator's preview of him answers as his own view does,
/// and the memory panel's rule for an identity's scope (which every
/// row and record of the panel passes before it leaves the server) shows
/// him nothing of its memory while it shows Alice; through the console Bob
/// is not answered the member's status and neither lists, pauses nor
/// deletes a schedule defined for it, and Alice lists it. Each message
/// starts with `case`.
async fn refused_through_the_console(
    runtime: &MobRuntime,
    service: &meerkat::ScheduleService,
    config: &AccessControlConfig,
    member: &str,
    case: &str,
) -> TestResult<Vec<String>> {
    use super::console_private_side_chat_tests::{
        alice_claims, bearer, bob_claims, enforced_console,
    };
    const BOB: &str = "bob@example.test";
    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(runtime, &access).await;
    let view = access.view_for_caller(Some(BOB), Some("person:bob"));
    if !view.knows_agent(member) {
        wrong.push(format!("{case}: the access cache does not know the member"));
    }
    for action in [ACTION_AGENT_VIEW, crate::access::ACTION_AGENT_SEND] {
        if view.allows_agent(action, member) {
            wrong.push(format!("{case}: the refused person holds {action}"));
        }
    }
    if access
        .decision_fence(Some(BOB), Some("person:bob"))
        .can_see_origin(member, None)
    {
        wrong.push(format!(
            "{case}: the decision fence shows the refused person a request of the member"
        ));
    }
    // The decision `meta/access/preview` answers an administrator with.
    let previewed = access
        .admin_preview(Some("root@example.test"), Some(BOB))
        .map(|preview| {
            [ACTION_AGENT_VIEW, crate::access::ACTION_AGENT_SEND]
                .map(|action| preview.decide_agent(action, member).is_allow())
        });
    if !matches!(previewed, Ok([false, false])) {
        wrong.push(format!(
            "{case}: an administrator's preview of the refused person does not answer \
             as the person's own view does"
        ));
    }
    if memory_panel_identity_visible(MemoryPanelCaller::own(Some(&view)), member) {
        wrong.push(format!(
            "{case}: the refused person is shown the member's memory scope"
        ));
    }
    let unrefused = access.view_for_caller(Some("alice@example.test"), Some("person:alice"));
    if !memory_panel_identity_visible(MemoryPanelCaller::own(Some(&unrefused)), member) {
        wrong.push(format!(
            "{case}: a person no rule refuses is not shown the member's memory scope"
        ));
    }

    let schedule_id = schedule_for(service, member).await?;
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let mut console = enforced_console(runtime, Some(AccessController::new(config.clone())?));
    console.console_aggregator = Some(aggregator);
    let app = console_json_router_with_state(console);
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let own = json!({ "identity": member, "schedule_id": schedule_id });
    let status = rpc_answer(
        &app,
        &bob,
        "meta/member_status",
        json!({ "member_id": member }),
    )
    .await;
    if !status["result"]["status"].is_null() {
        wrong.push(format!(
            "{case}: the refused person is answered the member's status: {status}"
        ));
    }
    let listed = schedule_answer(&app, &bob, "meta/schedule/list", member, &schedule_id).await;
    if listed.contains("scheduled-for-MEMBER") {
        wrong.push(format!(
            "{case}: the refused person lists the schedule: {listed}"
        ));
    }
    let paused = rpc_answer(&app, &bob, "meta/schedule/pause", own.clone()).await;
    if paused["result"]["phase"] == json!("paused") {
        wrong.push(format!(
            "{case}: the refused person pauses the schedule: {paused}"
        ));
    }
    let deleted = rpc_answer(&app, &bob, "meta/schedule/delete", own).await;
    if deleted["result"]["deleted"] == json!(true) {
        wrong.push(format!(
            "{case}: the refused person deletes the schedule: {deleted}"
        ));
    }
    let listed = schedule_answer(&app, &alice, "meta/schedule/list", member, &schedule_id).await;
    if !listed.contains("scheduled-for-MEMBER") {
        wrong.push(format!(
            "{case}: a person no rule refuses does not read the schedule: {listed}"
        ));
    }
    Ok(wrong)
}

/// A spawn an agent's tool made, as the console's spawn registry records
/// it: `member` of `mob_id`, spawned by `spawned_by` with `labels`.
async fn registered_spawn(
    sink: &crate::console_spawn::ConsoleSpawnSink,
    mob_id: &str,
    member: &str,
    spawned_by: &str,
    labels: BTreeMap<String, String>,
) {
    sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
        mob_id: Some(mob_id.to_string()),
        member_id: member.to_string(),
        identity: member.to_string(),
        initial_message: None,
        labels,
        spawned_by: Some(spawned_by.to_string()),
        spawned_by_session: None,
        session_id: None,
        via_tool: "mob_spawn_member".to_string(),
    })
    .await;
}

/// The person no rule refuses.
const ALICE: &str = "alice@example.test";
/// The person a rule denies the member by its role, or by a label it
/// carried, and whose other rule allows every agent.
const BOB: &str = "bob@example.test";
/// The person a rule denies the member by its name, and whose other rule
/// allows every agent.
const CAROL: &str = "carol@example.test";
/// The access administrator.
const ROOT: &str = "root@example.test";
/// The member of the rows on the console's doors: a worker of the console's
/// own mob whose roster row carries the label `tier = vault`.
const ENDED: &str = "own-helper";

/// The label the member of the rows on the console's doors carries.
fn vault_tier() -> BTreeMap<String, String> {
    BTreeMap::from([("tier".to_string(), "vault".to_string())])
}

/// What the rule that refuses the second person selects the member by.
#[derive(Clone, Copy)]
enum DeniedBy {
    /// The member's role in its mob.
    Role,
    /// A label the member's roster row carried.
    Label,
}

impl DeniedBy {
    fn words(self) -> &'static str {
        match self {
            Self::Role => "its role",
            Self::Label => "a label it carried",
        }
    }
}

/// Whether the gateway that seated the member still runs when the row asks
/// about the ended member.
#[derive(Clone, Copy)]
enum Gateway {
    /// It still runs: the same console is asked before and after the end.
    StillRunning,
    /// It was stopped and started again over the same state.
    Restarted,
}

impl Gateway {
    fn words(self) -> &'static str {
        match self {
            Self::StillRunning => "the gateway still running",
            Self::Restarted => "after a restart of the gateway",
        }
    }
}

/// A door of the console that reads or acts on one member.
#[derive(Clone, Copy)]
enum Door {
    /// The memory panel: the member's records listed by its name, its rows
    /// in the list of every scope, and one record read by id.
    MemoryPanel,
    /// The console schedules of the member: list, pause, delete.
    Schedules,
    /// `meta/member_status`.
    MemberStatus,
    /// `meta/access/preview`, asked by the administrator.
    AccessPreview,
    /// One method of each other family the shared method check decides for
    /// a member: the member's memory by RPC, the status of a send to it, the
    /// tool grants on it. What the door serves there is the method's own
    /// answer, whatever it is, in place of the access refusal.
    OtherFamilies,
}

impl Door {
    /// How many things the door serves a person it refuses nothing.
    fn serves(self) -> usize {
        match self {
            Self::MemoryPanel | Self::Schedules | Self::AccessPreview | Self::OtherFamilies => 3,
            Self::MemberStatus => 1,
        }
    }
}

/// The access rules of the rows on the console's doors: the first person
/// may do everything to every agent; so may the second, who is denied the
/// member by `denied_by`; so may the third, who is denied the member by its
/// name.
fn rules_of_the_door_rows(denied_by: DeniedBy) -> AccessControlConfig {
    let every_agent = |id: &str, subject: &str| crate::access::AccessRule {
        agents: vec!["*".to_string()],
        ..agent_rule(id, subject)
    };
    let by_what_the_member_carries = match denied_by {
        DeniedBy::Role => crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
        DeniedBy::Label => crate::access::AccessRule {
            match_labels: vault_tier(),
            ..agent_deny("bob-is-denied-the-vault-tier", BOB)
        },
    };
    alice_drives_every_agent_and(vec![
        every_agent("bob-drives-every-agent", BOB),
        by_what_the_member_carries,
        every_agent("carol-drives-every-agent", CAROL),
        crate::access::AccessRule {
            agents: vec![ENDED.to_string()],
            ..agent_deny("carol-is-denied-the-member-by-name", CAROL)
        },
    ])
}

/// The token of the signed-in `person`.
fn token_of(person: &str) -> String {
    let name = person.split('@').next().unwrap_or_default();
    super::console_private_side_chat_tests::bearer(json!({
        "sub": format!("oidc-sub-{name}"),
        "email": person,
        "provider": "google_oauth",
    }))
}

/// The title of the note the memory store holds in `member`'s scope.
fn note_title(member: &str) -> String {
    format!("{member} keeps a note")
}

/// `runtime` stopped and composed again over its state directory `state`,
/// as a gateway restart composes it, once its record of who its members are
/// private to has read every durable record.
async fn restarted(
    runtime: MobRuntime,
    state: &std::path::Path,
    mob: &str,
) -> TestResult<MobRuntime> {
    use super::console_side_chat_tests::persistent_gateway_runtime_in;
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + std::time::Duration::from_secs(90))
        .await;
    drop(runtime);
    let runtime = persistent_gateway_runtime_in(state, mob, "", "").await?;
    let handle = runtime.handle();
    assert!(
        runtime.private_owners().seeded(&handle).await.landed(),
        "the restarted runtime reads who its members are private to"
    );
    Ok(runtime)
}

/// A console of one runtime with sign-in for the four people and the
/// administrator, under the rows' access rules, with an aggregator over the
/// runtime, the memory panel over the rows' memory store and the console
/// schedules of `service`.
struct DoorConsole {
    app: Router,
    service: meerkat::ScheduleService,
    /// The id of the note in the member's memory scope.
    note_id: String,
}

fn door_console(
    runtime: &MobRuntime,
    config: &AccessControlConfig,
    memory: &Arc<dyn crate::memory::capabilities::MemoryPanelStore>,
    note_id: &str,
) -> TestResult<DoorConsole> {
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let mut console = super::console_private_side_chat_tests::enforced_console(
        runtime,
        Some(AccessController::new(config.clone())?),
    );
    console
        .decisions
        .auth
        .email_allowlist
        .extend([CAROL.to_string(), DAVE.to_string()]);
    console.console_aggregator = Some(aggregator);
    console.memory_panel = Some(Arc::clone(memory));
    Ok(DoorConsole {
        app: console_json_router_with_state(console),
        service,
        note_id: note_id.to_string(),
    })
}

/// What `door` serves `person` of the member [`ENDED`] through the console:
/// one line for each thing served, none when the door refuses the person.
async fn served(console: &DoorConsole, door: Door, person: &str) -> TestResult<Vec<String>> {
    use super::console_token_lapse_tests::console_rpc_as;
    let token = token_of(person);
    let mut served = Vec::new();
    match door {
        Door::MemoryPanel => {
            for (what, method, params) in [
                (
                    "reads its records, listed by its name",
                    "meta/memory/panel/records",
                    json!({ "identity": ENDED }),
                ),
                (
                    "reads its records in the list of every scope",
                    "meta/memory/panel/records",
                    json!({}),
                ),
                (
                    "reads its record by id",
                    "meta/memory/panel/record",
                    json!({ "memory_id": console.note_id }),
                ),
            ] {
                let answer = console_rpc_as(&console.app, Some(&token), method, params).await;
                if answer.to_string().contains(&note_title(ENDED)) {
                    served.push(what.to_string());
                }
            }
        }
        Door::Schedules => {
            // A schedule of its own for each person asked: a pause or a
            // delete one person is served changes nothing the next is asked.
            let schedule_id = schedule_for(&console.service, ENDED).await?;
            let own = json!({ "identity": ENDED, "schedule_id": schedule_id });
            let listed = console_rpc_as(
                &console.app,
                Some(&token),
                "meta/schedule/list",
                json!({ "identity": ENDED }),
            )
            .await;
            if listed
                .to_string()
                .contains(&format!("scheduled-for-{ENDED}"))
            {
                served.push("lists its schedule".to_string());
            }
            let paused = console_rpc_as(
                &console.app,
                Some(&token),
                "meta/schedule/pause",
                own.clone(),
            )
            .await;
            if paused["result"]["phase"] == json!("paused") {
                served.push("pauses its schedule".to_string());
            }
            let deleted =
                console_rpc_as(&console.app, Some(&token), "meta/schedule/delete", own).await;
            if deleted["result"]["deleted"] == json!(true) {
                served.push("deletes its schedule".to_string());
            }
        }
        Door::MemberStatus => {
            let answer = console_rpc_as(
                &console.app,
                Some(&token),
                "meta/member_status",
                json!({ "member_id": ENDED }),
            )
            .await;
            if !answer["result"]["status"].is_null() {
                served.push(format!(
                    "is answered its status ({})",
                    answer["result"]["status"]
                ));
            }
        }
        Door::OtherFamilies => {
            for (method, params) in [
                (
                    "meta/agent_memory/recall",
                    json!({
                        "identity": ENDED,
                        "selection": "contextual",
                        "query_text": "Where is the heron?",
                        "query_terms": ["heron"],
                    }),
                ),
                (
                    "meta/send_status",
                    json!({
                        "identity": ENDED,
                        "session_id": "01a1b8c0-96e4-71d4-8671-d7a73527e863",
                        "input_id": "00000000-0000-4000-8000-000000000001",
                    }),
                ),
                ("meta/tool_grants/list", json!({ "identity": ENDED })),
            ] {
                let answer = console_rpc_as(&console.app, Some(&token), method, params).await;
                if answer["error"]["code"] != json!(ACCESS_DENIED_RPC_CODE) {
                    served.push(format!("is answered {method} by the method"));
                }
            }
        }
        Door::AccessPreview => {
            let administrator = token_of(ROOT);
            for action in [
                ACTION_AGENT_VIEW,
                ACTION_AGENT_SEND,
                ACTION_AGENT_MEMORY_READ,
            ] {
                let answer = console_rpc_as(
                    &console.app,
                    Some(&administrator),
                    "meta/access/preview",
                    json!({ "subject": person, "action": action, "identity": ENDED }),
                )
                .await;
                match answer["result"]["allowed"].as_bool() {
                    Some(true) => served.push(format!("is previewed as allowed {action}")),
                    Some(false) => {}
                    None => {
                        return Err(
                            format!("the preview of {person} for {action}: {answer}").into()
                        );
                    }
                }
            }
        }
    }
    Ok(served)
}

/// What `door` serves each of the three people, in the order: the person no
/// rule refuses, the person denied the member by its role or label, the
/// person denied the member by its name.
async fn served_to_each(console: &DoorConsole, door: Door) -> TestResult<[Vec<String>; 3]> {
    Ok([
        served(console, door, ALICE).await?,
        served(console, door, BOB).await?,
        served(console, door, CAROL).await?,
    ])
}

/// The administrator's preview answers each person as the status door
/// answers that person: allowed the member's view exactly when the door
/// serves its status.
async fn the_preview_answers_as_the_status_door(console: &DoorConsole, when: &str) -> TestResult {
    for person in [ALICE, BOB, CAROL] {
        let previewed = served(console, Door::AccessPreview, person)
            .await?
            .iter()
            .any(|line| line.ends_with(ACTION_AGENT_VIEW));
        let at_the_door = !served(console, Door::MemberStatus, person)
            .await?
            .is_empty();
        assert_eq!(
            previewed, at_the_door,
            "control, {when}: the preview of {person} allows the member's view, and the status \
             door serves {person}"
        );
    }
    Ok(())
}

/// One row on a door of the console, for a member of the console's own mob
/// that ends. While the member lives, the door refuses the person a deny on
/// its role (or on a label it carries) applies to and the person denied it
/// by name, and serves the person no rule refuses. After the member's end
/// the three people are answered the same: the door still serves the person
/// no rule refuses, the deny on the name still holds, and the person denied
/// the member's role (or label), whose other rule allows every agent, is
/// served nothing of it.
async fn a_door_refuses_an_ended_member_as_it_refused_the_living_one(
    mob: &str,
    door: Door,
    denied_by: DeniedBy,
    gateway: Gateway,
) -> TestResult {
    use super::console_side_chat_tests::persistent_gateway_runtime_in;
    use crate::AgentMemoryProvider as _;

    let state = tempfile::tempdir()?;
    let memory_dir = tempfile::tempdir()?;
    let mut runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    // The planner is no worker and carries no label: nothing but the
    // member's own role and label decides the second person's deny.
    seat(&runtime.handle(), "lead", "planner", BTreeMap::new()).await?;
    seat(&runtime.handle(), "worker", ENDED, vault_tier()).await?;
    let store = crate::memory::SqliteAgentMemoryStore::open(memory_dir.path())?;
    let note = store
        .remember(
            &crate::AgentMemoryConfig::default().realm,
            &crate::identity_first::AgentIdentity::parse(ENDED)?,
            crate::NewAgentMemory {
                title: note_title(ENDED),
                body: "the heron stands in the reeds".to_string(),
                tags: Vec::new(),
            },
        )
        .await?;
    let memory: Arc<dyn crate::memory::capabilities::MemoryPanelStore> = Arc::new(store);
    let config = rules_of_the_door_rows(denied_by);
    let mut console = door_console(&runtime, &config, &memory, &note.memory_id)?;

    // While the member lives.
    let [unrefused, by_what_it_carries, by_name] = served_to_each(&console, door).await?;
    assert_eq!(
        unrefused.len(),
        door.serves(),
        "control, while the member lives: the person no rule refuses is served: {unrefused:?}"
    );
    assert!(
        by_what_it_carries.is_empty(),
        "control, while the member lives: the person denied {} is refused, but \
         {by_what_it_carries:?}",
        denied_by.words()
    );
    assert!(
        by_name.is_empty(),
        "control, while the member lives: the person denied its name is refused, but {by_name:?}"
    );
    if let Door::AccessPreview = door {
        the_preview_answers_as_the_status_door(&console, "while the member lives").await?;
    }

    // The member ends.
    retired(&runtime.handle(), ENDED).await?;
    if let Gateway::Restarted = gateway {
        drop(console);
        runtime = restarted(runtime, state.path(), mob).await?;
        console = door_console(&runtime, &config, &memory, &note.memory_id)?;
    }
    assert!(
        runtime
            .handle()
            .get_member(&AgentIdentity::from(ENDED))
            .await?
            .is_none(),
        "the member has ended"
    );

    let [unrefused, by_what_it_carries, by_name] = served_to_each(&console, door).await?;
    assert_eq!(
        unrefused.len(),
        door.serves(),
        "control, the member ended, {}: the person no rule refuses is still served: {unrefused:?}",
        gateway.words()
    );
    assert!(
        by_name.is_empty(),
        "control, the member ended, {}: the person denied its name stays refused, but {by_name:?}",
        gateway.words()
    );
    if let Door::AccessPreview = door {
        the_preview_answers_as_the_status_door(&console, "the member ended").await?;
    }
    assert!(
        by_what_it_carries.is_empty(),
        "the member ended, {}: the person denied {} is served after the member's end: \
         {by_what_it_carries:?}",
        gateway.words(),
        denied_by.words()
    );
    Ok(())
}

/// The memory panel shows nothing of an ended member to a person a rule
/// denies the member's role, whose other rule allows every agent: not its
/// records listed by its name, not its rows in the list of every scope, not
/// a record of it read by id. The gateway that seated the member still
/// runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_memory_is_refused_to_a_person_denied_its_role() -> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-memory-by-role",
        Door::MemoryPanel,
        DeniedBy::Role,
        Gateway::StillRunning,
    )
    .await
}

/// [`an_ended_members_memory_is_refused_to_a_person_denied_its_role`] after
/// a restart of the gateway on the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_memory_is_refused_to_a_person_denied_its_role_after_a_restart()
-> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-memory-by-role-restarted",
        Door::MemoryPanel,
        DeniedBy::Role,
        Gateway::Restarted,
    )
    .await
}

/// The memory panel shows nothing of an ended member to a person a rule
/// denies a label the member's roster row carried, whose other rule allows
/// every agent. The gateway that seated the member still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_memory_is_refused_to_a_person_denied_a_label_it_carried() -> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-memory-by-label",
        Door::MemoryPanel,
        DeniedBy::Label,
        Gateway::StillRunning,
    )
    .await
}

/// [`an_ended_members_memory_is_refused_to_a_person_denied_a_label_it_carried`]
/// after a restart of the gateway on the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_memory_is_refused_to_a_person_denied_a_label_it_carried_after_a_restart()
-> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-memory-by-label-restarted",
        Door::MemoryPanel,
        DeniedBy::Label,
        Gateway::Restarted,
    )
    .await
}

/// A person a rule denies an ended member's role, whose other rule allows
/// every agent, neither lists, pauses nor deletes a console schedule
/// defined for the member. The gateway that seated the member still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_console_schedules_are_refused_to_a_person_denied_its_role() -> TestResult
{
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-schedules-by-role",
        Door::Schedules,
        DeniedBy::Role,
        Gateway::StillRunning,
    )
    .await
}

/// [`an_ended_members_console_schedules_are_refused_to_a_person_denied_its_role`]
/// after a restart of the gateway on the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_console_schedules_are_refused_to_a_person_denied_its_role_after_a_restart()
-> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-schedules-by-role-restarted",
        Door::Schedules,
        DeniedBy::Role,
        Gateway::Restarted,
    )
    .await
}

/// `meta/member_status` answers nothing of an ended member to a person a
/// rule denies the member's role, whose other rule allows every agent. The
/// gateway that seated the member still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_status_is_refused_to_a_person_denied_its_role() -> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-status-by-role",
        Door::MemberStatus,
        DeniedBy::Role,
        Gateway::StillRunning,
    )
    .await
}

/// [`an_ended_members_status_is_refused_to_a_person_denied_its_role`] after
/// a restart of the gateway on the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_status_is_refused_to_a_person_denied_its_role_after_a_restart()
-> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-status-by-role-restarted",
        Door::MemberStatus,
        DeniedBy::Role,
        Gateway::Restarted,
    )
    .await
}

/// The shared method check refuses a person a rule denies an ended
/// member's role, whose other rule allows every agent, one method of each
/// other family it decides for the member: its memory by RPC, the status
/// of a send to it, the tool grants on it. The gateway that seated the
/// member still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_other_methods_are_refused_to_a_person_denied_its_role() -> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-families-by-role",
        Door::OtherFamilies,
        DeniedBy::Role,
        Gateway::StillRunning,
    )
    .await
}

/// [`an_ended_members_other_methods_are_refused_to_a_person_denied_its_role`]
/// after a restart of the gateway on the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_members_other_methods_are_refused_to_a_person_denied_its_role_after_a_restart()
-> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-families-by-role-restarted",
        Door::OtherFamilies,
        DeniedBy::Role,
        Gateway::Restarted,
    )
    .await
}

/// The administrator's `meta/access/preview` answers for an ended member
/// as the doors answer the previewed person: a person a rule denies the
/// member's role, whose other rule allows every agent, is previewed as
/// refused its view, a send to it and its memory. The gateway that seated
/// the member still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_access_preview_answers_an_ended_member_as_the_doors_do() -> TestResult {
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-preview-by-role",
        Door::AccessPreview,
        DeniedBy::Role,
        Gateway::StillRunning,
    )
    .await
}

/// [`the_access_preview_answers_an_ended_member_as_the_doors_do`] after a
/// restart of the gateway on the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_access_preview_answers_an_ended_member_as_the_doors_do_after_a_restart() -> TestResult
{
    a_door_refuses_an_ended_member_as_it_refused_the_living_one(
        "ended-preview-by-role-restarted",
        Door::AccessPreview,
        DeniedBy::Role,
        Gateway::Restarted,
    )
    .await
}

/// A member the host seats through the mob handle, and a member an agent
/// spawns with the mob's own member tool `spawn_member`, leave nothing in
/// the console's spawn registry: only `mob_spawn_member` writes it. Once
/// such a member ended, with the gateway that seated it still running, the
/// console knows it from its record of who each member is private to alone,
/// which holds its name and its owner and neither its role nor its labels.
/// A deny on its role decides it closed all the same: the person the rule
/// applies to, whose other rule allows every agent, has no view and no send
/// on it, is not shown its memory scope or its status, sees no request of
/// it at the decision fence, and neither lists, pauses nor deletes its
/// console schedule, while a person no rule refuses reads all of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deny_by_role_refuses_an_ended_member_no_spawn_record_names() -> TestResult {
    const BOB: &str = "bob@example.test";
    const MEMBER: &str = "handle-helper";
    let root = tempfile::tempdir()?;
    let runtime = runtime("no-spawn-record", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", MEMBER, BTreeMap::new()).await?;
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
    ]);
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    assert!(
        !access
            .view_for_caller(Some(BOB), Some("person:bob"))
            .can_view_agent(MEMBER),
        "while it is seated the person denied the workers does not see the member"
    );

    retired(&primary, MEMBER).await?;
    assert!(
        !runtime
            .console_identity_labels()
            .await
            .holds_identity(MEMBER),
        "the spawn registry never held the member"
    );
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    let wrong = refused_through_the_console(
        &runtime,
        &service,
        &config,
        MEMBER,
        "no spawn record, ended, the gateway still running",
    )
    .await?;
    assert!(
        wrong.is_empty(),
        "an ended member no spawn record names: {wrong:#?}"
    );
    Ok(())
}

/// A member an agent's tool spawned keeps an entry in the console's spawn
/// registry after it ended, until the gateway starts again. What the
/// registry holds is of the last spawn the tool made under the name: the
/// labels the call carried and who made it, never the member's role in
/// its mob, and not necessarily of the member that ended. So nothing of
/// it decides the ended member: the console knows the member's name and
/// who it was private to, the same before a restart and after one. A
/// person a rule denies the member's role, or the agent that spawned it,
/// stays refused: no view, no send, no request of it at the
/// decision fence, and its console schedule is not listed to, paused or
/// deleted by that person. A grant on the spawner, a grant by a label the
/// spawn carried and a grant by its role show nothing of the ended
/// member. A label named `role` that the spawn carried is a label and
/// nothing more: a grant on that value as a role shows nothing, seated or
/// ended. A member the ended one spawned is decided through it: what is
/// not known of the ended one is not known of its lineage.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_a_spawn_recorded_decides_an_ended_member_the_spawn_registry_holds() -> TestResult {
    const ALICE: &str = "alice@example.test";
    const BOB: &str = "bob@example.test";
    const DAVE: &str = "dave@example.test";
    const ERIN: &str = "erin@example.test";
    const FRANK: &str = "frank@example.test";
    const GRACE: &str = "grace@example.test";
    const HEIDI: &str = "heidi@example.test";
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);

    let root = tempfile::tempdir()?;
    let runtime = runtime("registry-held", root.path()).await?;
    let primary = runtime.handle();
    // The planner is no worker: nothing but the member's own role decides
    // a rule on the workers.
    seat(&primary, "delegate", "planner", BTreeMap::new()).await?;
    let team = runtime.handle();
    // A worker of the vault tier whose spawn also carried a label named
    // `role`, and a member that worker spawned in turn.
    let spawn_labels = BTreeMap::from([
        ("tier".to_string(), "vault".to_string()),
        ("role".to_string(), "auditor".to_string()),
    ]);
    seat(&team, "worker", "helper", spawn_labels.clone()).await?;
    seat(&team, "delegate", "apprentice", BTreeMap::new()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::new(ConsoleEventStore::new());
    runtime.install_console_spawn_sink(sink.clone());
    let helper = "helper".to_string();
    let apprentice = "apprentice".to_string();
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        "helper",
        "planner",
        spawn_labels,
    )
    .await;
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        "apprentice",
        &helper,
        BTreeMap::new(),
    )
    .await;

    // Bob is allowed what the planner spawned and denied the workers. Dave
    // is allowed what the planner spawned, Erin the vault tier, Frank the
    // auditors, Grace the workers. Heidi may do everything to every agent
    // and is denied what the planner spawned.
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_rule("bob-is-granted-the-planners-members", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_rule("dave-is-granted-the-planners-members", DAVE)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "vault"),
            ..agent_rule("erin-is-granted-the-vault", ERIN)
        },
        crate::access::AccessRule {
            roles: vec!["auditor".to_string()],
            ..agent_rule("frank-is-granted-the-auditors", FRANK)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_rule("grace-is-granted-the-workers", GRACE)
        },
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("heidi-drives-every-agent", HEIDI)
        },
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_deny("heidi-is-denied-the-planners-members", HEIDI)
        },
    ]);
    let sees = |access: &AccessController, subject: &str, member: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(member)
    };
    let people = [ALICE, BOB, DAVE, ERIN, FRANK, GRACE, HEIDI];

    let mut wrong = Vec::new();
    // While the member is seated its roster row decides it.
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let seated = people.map(|subject| sees(&access, subject, &helper));
    if seated != [true, false, true, true, false, true, false] {
        wrong.push(format!(
            "seated, alice, bob, dave, erin, frank, grace and heidi see the member as {seated:?}"
        ));
    }
    if sees(&access, BOB, &apprentice) || !sees(&access, DAVE, &apprentice) {
        wrong.push("seated: the member it spawned is not decided through it".to_string());
    }

    // The member ends; the gateway keeps running and the registry keeps it.
    retired(&team, "helper").await?;
    assert!(
        runtime
            .console_identity_labels()
            .await
            .holds_identity(&helper),
        "the spawn registry still holds the ended member"
    );
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    // Only its name and its owner are known: an allow on every
    // agent still shows it, every allow that reads more grants nothing,
    // and every deny that reads more applies.
    let ended = people.map(|subject| sees(&access, subject, &helper));
    if ended != [true, false, false, false, false, false, false] {
        wrong.push(format!(
            "ended, alice, bob, dave, erin, frank, grace and heidi see the member as {ended:?}"
        ));
    }
    if sees(&access, BOB, &apprentice) {
        wrong.push(
            "ended: the person denied the member's role sees the member it spawned".to_string(),
        );
    }
    if sees(&access, DAVE, &apprentice) {
        wrong.push(
            "ended: a grant above the ended member reaches the member it spawned".to_string(),
        );
    }
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    wrong.extend(
        refused_through_the_console(&runtime, &service, &config, &helper, "a deny by role").await?,
    );
    assert!(
        wrong.is_empty(),
        "an ended member the spawn registry holds: {wrong:#?}"
    );
    Ok(())
}

/// The console's spawn registry holds the labels a spawn call carried. A
/// label the member's roster row carried besides (a host's spawn customizer
/// gave it) is not among them, and once the member ended nothing holds it:
/// it is not known, which is not the same as "the member did not carry it".
/// So a deny on such a label refuses the ended member while the gateway
/// that seated it still runs, as it did while the member was seated and as
/// it does after a restart, and an allow on such a label grants nothing. A
/// label the spawn call did carry is the call's value, not the row's: it
/// decides nothing for the ended member either, so a deny on it applies
/// and an allow on it grants nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deny_by_a_label_the_spawn_did_not_carry_refuses_an_ended_member_the_spawn_registry_holds()
-> TestResult {
    const BOB: &str = "bob@example.test";
    const DAVE: &str = "dave@example.test";
    const ERIN: &str = "erin@example.test";
    const FRANK: &str = "frank@example.test";
    const MEMBER: &str = "labelled-helper";
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
    let root = tempfile::tempdir()?;
    let runtime = runtime("label-not-in-the-spawn", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "delegate", "planner", BTreeMap::new()).await?;
    // The roster row carries two labels; the spawn call carried one.
    seat(
        &primary,
        "worker",
        MEMBER,
        BTreeMap::from([
            ("tier".to_string(), "vault".to_string()),
            ("team".to_string(), "blue".to_string()),
        ]),
    )
    .await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::new(ConsoleEventStore::new());
    runtime.install_console_spawn_sink(sink.clone());
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        MEMBER,
        "planner",
        label("team", "blue"),
    )
    .await;

    // Bob may do everything to every agent and is denied the vault tier.
    // Dave is granted the blue team, Erin the vault tier. Frank may do
    // everything to every agent and is denied the blue team, the label
    // the spawn call carried.
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "vault"),
            ..agent_deny("bob-is-denied-the-vault", BOB)
        },
        crate::access::AccessRule {
            match_labels: label("team", "blue"),
            ..agent_rule("dave-is-granted-the-blue-team", DAVE)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "vault"),
            ..agent_rule("erin-is-granted-the-vault", ERIN)
        },
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("frank-drives-every-agent", FRANK)
        },
        crate::access::AccessRule {
            match_labels: label("team", "blue"),
            ..agent_deny("frank-is-denied-the-blue-team", FRANK)
        },
    ]);
    let sees = |access: &AccessController, subject: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(MEMBER)
    };
    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let seated = [BOB, DAVE, ERIN, FRANK].map(|subject| sees(&access, subject));
    if seated != [false, true, true, false] {
        wrong.push(format!(
            "seated, bob, dave, erin and frank see the member as {seated:?}"
        ));
    }

    retired(&primary, MEMBER).await?;
    assert!(
        runtime
            .console_identity_labels()
            .await
            .holds_identity(MEMBER),
        "the spawn registry still holds the ended member"
    );
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let ended = [BOB, DAVE, ERIN, FRANK].map(|subject| sees(&access, subject));
    if ended != [false, false, false, false] {
        wrong.push(format!(
            "ended, bob, dave, erin and frank see the member as {ended:?}"
        ));
    }
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    wrong.extend(
        refused_through_the_console(
            &runtime,
            &service,
            &config,
            MEMBER,
            "a deny by a label the spawn did not carry",
        )
        .await?,
    );
    assert!(
        wrong.is_empty(),
        "an ended member whose roster row carried a label its spawn did not: {wrong:#?}"
    );
    Ok(())
}

/// What outlives a member in the console's spawn registry is of the last
/// spawn made under the name, not of the member that ended: an ended
/// member is decided by nothing an earlier member of its name recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_member_is_not_decided_by_an_earlier_member_of_its_name() -> TestResult {
    const BOB: &str = "bob@example.test";
    const DAVE: &str = "dave@example.test";
    const ERIN: &str = "erin@example.test";
    const FRANK: &str = "frank@example.test";
    const MEMBER: &str = "reseated";
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
    let root = tempfile::tempdir()?;
    let runtime = runtime("name-reuse", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::new(ConsoleEventStore::new());
    runtime.install_console_spawn_sink(sink.clone());
    // The first member: a worker of the open tier, spawned by the planner's
    // tool. It ends.
    seat(&primary, "worker", MEMBER, label("tier", "open")).await?;
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        MEMBER,
        "planner",
        label("tier", "open"),
    )
    .await;
    retired(&primary, MEMBER).await?;
    // The second member of the name: seated by the host, another role,
    // the sealed tier. Nothing registers it. It ends too.
    seat(&primary, "delegate", MEMBER, label("tier", "sealed")).await?;

    // Bob may do everything to every agent and is denied the sealed tier;
    // Frank the same, denied the delegates. Dave is granted what the
    // planner spawned, Erin the open tier.
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "sealed"),
            ..agent_deny("bob-is-denied-the-sealed-tier", BOB)
        },
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_rule("dave-is-granted-the-planners-members", DAVE)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "open"),
            ..agent_rule("erin-is-granted-the-open-tier", ERIN)
        },
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("frank-drives-every-agent", FRANK)
        },
        crate::access::AccessRule {
            roles: vec!["delegate".to_string()],
            ..agent_deny("frank-is-denied-the-delegates", FRANK)
        },
    ]);
    let sees = |access: &AccessController, subject: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(MEMBER)
    };
    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    // While the second member is seated its own row decides the denies.
    let seated = [BOB, FRANK].map(|subject| sees(&access, subject));
    if seated != [false, false] {
        wrong.push(format!(
            "the second member seated, bob and frank see it as {seated:?}"
        ));
    }

    retired(&primary, MEMBER).await?;
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let ended = [BOB, DAVE, ERIN, FRANK].map(|subject| sees(&access, subject));
    if ended != [false, false, false, false] {
        wrong.push(format!(
            "the second member ended, bob, dave, erin and frank see it as {ended:?}"
        ));
    }
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    wrong.extend(
        refused_through_the_console(
            &runtime,
            &service,
            &config,
            MEMBER,
            "a deny on the second member's label",
        )
        .await?,
    );
    assert!(
        wrong.is_empty(),
        "an ended member under a name an earlier member held: {wrong:#?}"
    );
    Ok(())
}

/// An ended member's name is known, so a grant on that name still decides:
/// it shows the ended member, and it reaches a living member the ended one
/// spawned, whose own row names its spawner. A grant on an agent above the
/// ended member reached that living member while its spawner lived and no
/// longer does: who spawned the ended member is not known.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grant_on_an_ended_members_own_name_still_reaches_the_member_it_spawned() -> TestResult {
    const DAVE: &str = "dave@example.test";
    const IVAN: &str = "ivan@example.test";

    let root = tempfile::tempdir()?;
    let runtime = runtime("named-spawner", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "delegate", "planner", BTreeMap::new()).await?;
    let team = runtime.handle();
    seat(&team, "worker", "helper", BTreeMap::new()).await?;
    seat(&team, "delegate", "apprentice", BTreeMap::new()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::new(ConsoleEventStore::new());
    runtime.install_console_spawn_sink(sink.clone());
    let helper = "helper".to_string();
    let apprentice = "apprentice".to_string();
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        "helper",
        "planner",
        BTreeMap::new(),
    )
    .await;
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        "apprentice",
        &helper,
        BTreeMap::new(),
    )
    .await;

    // Dave is allowed what the planner spawned, Ivan what the helper
    // spawned (and the helper itself).
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_rule("dave-is-granted-the-planners-members", DAVE)
        },
        crate::access::AccessRule {
            agents: vec![helper.clone()],
            ..agent_rule("ivan-is-granted-the-helper-and-its-members", IVAN)
        },
    ]);
    let sees = |access: &AccessController, subject: &str, member: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(member)
    };

    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let seated = [
        sees(&access, DAVE, &helper),
        sees(&access, DAVE, &apprentice),
        sees(&access, IVAN, &helper),
        sees(&access, IVAN, &apprentice),
    ];
    if seated != [true, true, true, true] {
        wrong.push(format!(
            "seated, dave and ivan see the helper and the member it spawned as {seated:?}"
        ));
    }

    retired(&team, "helper").await?;
    let access = AccessController::new(config)?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let ended = [
        sees(&access, DAVE, &helper),
        sees(&access, DAVE, &apprentice),
        sees(&access, IVAN, &helper),
        sees(&access, IVAN, &apprentice),
    ];
    if ended != [false, false, true, true] {
        wrong.push(format!(
            "the helper ended, dave and ivan see it and the member it spawned as {ended:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a grant on an ended member's own name: {wrong:#?}"
    );
    Ok(())
}

/// An agent the console's access cache does not know is not judged as a
/// bare name where a rule that applies to the caller reads what a name does
/// not carry (a role, a label, another agent): a method that reads or acts
/// on an agent refuses such a caller, as the memory panel does, and the
/// refusal says nothing of the agent. An agent private to another person
/// and a name no agent holds are answered alike, each under the name as
/// the caller spelled it. A caller no such rule applies to is answered by
/// the method. A spawn is not refused so: it names an agent that does not
/// exist yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_method_on_an_agent_the_console_does_not_know_is_refused_alike_for_a_hidden_agent_and_for_no_agent()
-> TestResult {
    use super::console_private_side_chat_tests::{
        alice_claims, bearer, bob_claims, enforced_console, fork_on_console,
    };
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
    const BOB: &str = "bob@example.test";
    const HIDDEN: &str = "alpha-side";
    const NOBODY: &str = "never-recorded";
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
    ]);
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "unknown-agent", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(
        &runtime,
        Some(AccessController::new(config.clone())?),
    ));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    // A side chat private to Alice: hidden from Bob.
    let forked = fork_on_console(&app, Some(&alice), "alpha", HIDDEN).await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let mut wrong = Vec::new();
    let hidden = rpc_answer(
        &app,
        &bob,
        "meta/member_status",
        json!({ "member_id": HIDDEN }),
    )
    .await;
    let absent = rpc_answer(
        &app,
        &bob,
        "meta/member_status",
        json!({ "member_id": NOBODY }),
    )
    .await;
    if absent["error"]["code"] != json!(ACCESS_DENIED_RPC_CODE)
        || absent["error"]["data"]["resource"] != json!(NOBODY)
    {
        wrong.push(format!(
            "the status of a name no agent holds, for a person a deny by role applies to: {absent}"
        ));
    }
    if hidden.to_string().replace(HIDDEN, "<member>")
        != absent.to_string().replace(NOBODY, "<member>")
    {
        wrong.push(format!(
            "an agent hidden from the person is not answered as a name no agent holds: hidden \
             {hidden}, absent {absent}"
        ));
    }
    let unrefused = rpc_answer(
        &app,
        &alice,
        "meta/member_status",
        json!({ "member_id": NOBODY }),
    )
    .await;
    if unrefused["error"]["code"] == json!(ACCESS_DENIED_RPC_CODE) {
        wrong.push(format!(
            "the same status, for a person no rule refuses: {unrefused}"
        ));
    }
    // A spawn names an agent that does not exist yet: the check does not
    // refuse it for what it cannot know of the name.
    let access = AccessController::new(config)?;
    let spawn = json!({
        "role": "worker",
        "agent_identity": "a-new-name",
        "session_id": SessionId::new().to_string(),
    });
    if let Some(error) = console_rpc_access_violation(
        Some(&access.view_for_caller(Some(BOB), Some("person:bob"))),
        "meta/attach_existing_session",
        &spawn,
    ) {
        wrong.push(format!(
            "a spawn under a name no agent holds yet: {}",
            error.message
        ));
    }
    assert!(
        wrong.is_empty(),
        "an agent the console does not know: {wrong:#?}"
    );
    Ok(())
}

/// A method that records under a member's name (a schedule, a remembered
/// fact) answers an agent hidden from the caller exactly as a name no agent
/// holds, each under the name as the caller spelled it: a caller a deny by
/// role applies to, and a caller no such rule applies to, whom the access
/// check does not refuse the name no agent holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_method_that_records_under_a_name_answers_a_hidden_agent_as_a_name_no_agent_holds()
-> TestResult {
    use super::console_private_side_chat_tests::{
        alice_claims, bearer, bob_claims, enforced_console, fork_on_console,
    };
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
    const BOB: &str = "bob@example.test";
    const HIDDEN: &str = "alpha-side";
    const NOBODY: &str = "never-recorded";
    /// The row pins that these methods answer a caller no deny applies to
    /// alike for an agent hidden from that caller and for a name no agent
    /// holds. With `false` it would pin the hidden agent answered as an
    /// unknown identity.
    const HIDDEN_IS_A_FREE_NAME_TO_EVERY_CALLER: bool = true;
    let bob_drives_every_agent = || crate::access::AccessRule {
        agents: vec!["*".to_string()],
        ..agent_rule("bob-drives-every-agent", BOB)
    };
    let under_a_deny = alice_drives_every_agent_and(vec![
        bob_drives_every_agent(),
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
    ]);
    let with_no_deny = alice_drives_every_agent_and(vec![bob_drives_every_agent()]);
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "recorded-name", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service,
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    let denied_console = console_json_router_with_state(enforced_console(
        &runtime,
        Some(AccessController::new(under_a_deny)?),
    ));
    let plain_console = console_json_router_with_state(enforced_console(
        &runtime,
        Some(AccessController::new(with_no_deny)?),
    ));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    // A side chat private to Alice: hidden from Bob on both consoles.
    let forked = fork_on_console(&denied_console, Some(&alice), "alpha", HIDDEN).await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let mut wrong = Vec::new();
    for method in ["meta/schedule/create", "meta/agent_memory/remember"] {
        // The person a deny by role applies to.
        let hidden = rpc_answer(&denied_console, &bob, method, json!({ "identity": HIDDEN })).await;
        let absent = rpc_answer(&denied_console, &bob, method, json!({ "identity": NOBODY })).await;
        if hidden.to_string().replace(HIDDEN, "<member>")
            != absent.to_string().replace(NOBODY, "<member>")
        {
            wrong.push(format!(
                "{method}, for the person a deny applies to: the hidden agent is not answered \
                 as a name no agent holds: hidden {hidden}, absent {absent}"
            ));
        }
        // The person no such rule applies to.
        let hidden = rpc_answer(&plain_console, &bob, method, json!({ "identity": HIDDEN })).await;
        let absent = rpc_answer(&plain_console, &bob, method, json!({ "identity": NOBODY })).await;
        if HIDDEN_IS_A_FREE_NAME_TO_EVERY_CALLER {
            if hidden.to_string().replace(HIDDEN, "<member>")
                != absent.to_string().replace(NOBODY, "<member>")
            {
                wrong.push(format!(
                    "{method}, for the person no rule refuses: the hidden agent is not answered \
                     as a name no agent holds: hidden {hidden}, absent {absent}"
                ));
            }
        } else if hidden["error"] != serde_json::to_value(unknown_identity_rpc_error(HIDDEN))? {
            wrong.push(format!(
                "{method}, for the person no rule refuses: the hidden agent is not answered as \
                 an unknown identity: {hidden}"
            ));
        }
        if absent["error"]["code"] == json!(ACCESS_DENIED_RPC_CODE) {
            wrong.push(format!(
                "{method}, for the person no rule refuses: a name no agent holds is refused by \
                 the access check: {absent}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a method that records under a member's name: {wrong:#?}"
    );
    Ok(())
}

/// A member seated in a mob the console does not list (a mob the host
/// created beside the console's own, for one) has no roster row the console
/// reads. Where a tool spawn registered it, the console knows its name and
/// nothing its spawn recorded: a person denied the workers, whose other
/// rule allows every agent, does not see it, and a grant by a label its
/// spawn carried shows nothing of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_of_a_mob_the_console_does_not_list_is_decided_closed() -> TestResult {
    const BOB: &str = "bob@example.test";
    const ERIN: &str = "erin@example.test";
    const MEMBER: &str = "outsider";
    let tier = || BTreeMap::from([("tier".to_string(), "vault".to_string())]);
    let root = tempfile::tempdir()?;
    let runtime = runtime("unlisted-mob", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "delegate", "planner", BTreeMap::new()).await?;
    let beside = mob_beside(&runtime, "unlisted-mob-beside").await?;
    seat(&beside, "worker", MEMBER, tier()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::new(ConsoleEventStore::new());
    runtime.install_console_spawn_sink(sink.clone());
    registered_spawn(&sink, "unlisted-mob-beside", MEMBER, "planner", tier()).await;
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
        crate::access::AccessRule {
            match_labels: tier(),
            ..agent_rule("erin-is-granted-the-vault", ERIN)
        },
    ]);
    let access = AccessController::new(config)?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let sees = |subject: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(MEMBER)
    };
    assert!(
        access
            .view_for_caller(Some(BOB), Some("person:bob"))
            .knows_agent(MEMBER),
        "the console knows the member by its spawn's registration"
    );
    assert_eq!(
        ["alice@example.test", BOB, ERIN].map(sees),
        [true, false, false],
        "alice, bob and erin see the member of the mob the console does not list"
    );
    Ok(())
}

/// A person whose one deny is on the view of the workers.
const DAVE: &str = "dave@example.test";

/// The methods that create a member under a name the caller gives.
const SPAWN_METHODS: [&str; 2] = ["meta/ensure_member", "meta/spawn_helper"];

/// The console of `runtime` with sign-in for the four people and the
/// administrator, under `config`, with an aggregator over the runtime.
fn signed_in_console(runtime: &MobRuntime, config: &AccessControlConfig) -> TestResult<Router> {
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let mut console = super::console_private_side_chat_tests::enforced_console(
        runtime,
        Some(AccessController::new(config.clone())?),
    );
    console
        .decisions
        .auth
        .email_allowlist
        .extend([CAROL.to_string(), DAVE.to_string()]);
    console.console_aggregator = Some(aggregator);
    Ok(console_json_router_with_state(console))
}

/// The answer of `method` to `person`, who creates a member of `role` named
/// `name` with it.
async fn spawn_answer(app: &Router, person: &str, method: &str, name: &str, role: &str) -> Value {
    use super::console_token_lapse_tests::console_rpc_as;
    let params = if method == "meta/ensure_member" {
        json!({ "role": role, "agent_identity": name })
    } else {
        json!({
            "agent_identity": name,
            "task": "Say ok.",
            "options": { "role": role },
            "result_label": "answer",
            "max_text_bytes": 1024,
        })
    };
    console_rpc_as(app, Some(&token_of(person)), method, params).await
}

/// Whether `answer` is the access refusal.
fn is_the_access_refusal(answer: &Value) -> bool {
    answer["error"]["code"] == json!(ACCESS_DENIED_RPC_CODE)
}

/// The name of the worker `person` is asked to create a member under with
/// `method`: one worker for each person and method, so that a member one
/// call creates changes nothing the next call is asked.
fn worker_for(person: &str, method: &str) -> String {
    format!(
        "worker-for-{}-{}",
        person.split('@').next().unwrap_or_default(),
        method
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .replace('_', "-")
    )
}

/// `member` retired from `mob` if it is seated there.
async fn ended_if_seated(mob: &MobHandle, member: &str) -> TestResult {
    if mob
        .get_member(&AgentIdentity::from(member))
        .await?
        .is_some()
    {
        retired(mob, member).await?;
    }
    Ok(())
}

/// A worker of the console's own mob for each of `people` and each spawn
/// method ([`worker_for`]), each carrying the vault tier label, each seated
/// and ended in turn.
async fn workers_that_ended(mob: &MobHandle, people: &[&str]) -> TestResult {
    for person in people {
        for method in SPAWN_METHODS {
            let name = worker_for(person, method);
            seat(mob, "worker", &name, vault_tier()).await?;
            retired(mob, &name).await?;
        }
    }
    Ok(())
}

/// A deny on `agent.spawn` applies to a spawn under the name of a worker of
/// the console's own mob that ended, as it applied while the worker lived:
/// a person who may do everything to every agent and is denied the spawn of
/// the workers by their role, and a person denied it by a label the worker
/// carried, are answered the access refusal by `meta/ensure_member` and by
/// `meta/spawn_helper` under the ended worker's name. A person no deny
/// applies to creates a member under an ended worker's name, and the two
/// people under a deny create a member under a name no agent ever held.
async fn a_spawn_under_an_ended_workers_name_is_refused_under_a_deny(
    mob: &str,
    gateway: Gateway,
) -> TestResult {
    use super::console_side_chat_tests::persistent_gateway_runtime_in;
    const LIVING: &str = "living-worker";
    let spawn_only = |rule: crate::access::AccessRule| crate::access::AccessRule {
        actions: vec![ACTION_AGENT_SPAWN.to_string()],
        ..rule
    };
    let every_agent = |id: &str, subject: &str| crate::access::AccessRule {
        agents: vec!["*".to_string()],
        ..agent_rule(id, subject)
    };
    let config = alice_drives_every_agent_and(vec![
        every_agent("bob-drives-every-agent", BOB),
        spawn_only(crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-spawn-of-the-workers", BOB)
        }),
        every_agent("carol-drives-every-agent", CAROL),
        spawn_only(crate::access::AccessRule {
            match_labels: vault_tier(),
            ..agent_deny("carol-is-denied-the-spawn-of-the-vault-tier", CAROL)
        }),
    ]);
    let denied = [(BOB, "its role"), (CAROL, "a label it carried")];

    let state = tempfile::tempdir()?;
    let mut runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    workers_that_ended(&runtime.handle(), &[ALICE, BOB, CAROL]).await?;
    seat(&runtime.handle(), "worker", LIVING, vault_tier()).await?;
    let mut app = signed_in_console(&runtime, &config)?;

    // While a worker lives, neither person creates a member under its name.
    for (person, by) in denied {
        for method in SPAWN_METHODS {
            let answer = spawn_answer(&app, person, method, LIVING, "worker").await;
            assert!(
                is_the_access_refusal(&answer),
                "control, while a worker lives: the person denied the spawn by {by} is refused \
                 {method} under its name: {answer}"
            );
        }
    }
    retired(&runtime.handle(), LIVING).await?;
    if let Gateway::Restarted = gateway {
        drop(app);
        runtime = restarted(runtime, state.path(), mob).await?;
        app = signed_in_console(&runtime, &config)?;
    }
    let handle = runtime.handle();
    let when = gateway.words();

    // A person no deny applies to creates a member under an ended worker's
    // name.
    for method in SPAWN_METHODS {
        let name = worker_for(ALICE, method);
        let answer = spawn_answer(&app, ALICE, method, &name, "worker").await;
        assert!(
            answer["error"].is_null(),
            "control, the workers ended, {when}: a person no deny applies to creates a member \
             under an ended worker's name with {method}: {answer}"
        );
        if method == "meta/ensure_member" {
            assert!(
                handle
                    .get_member(&AgentIdentity::from(name.as_str()))
                    .await?
                    .is_some(),
                "control, the workers ended, {when}: the member is seated under that name"
            );
        }
        ended_if_seated(&handle, &name).await?;
    }
    // The people under a deny create a member under a name no agent ever
    // held: a spawn names an agent that does not exist yet.
    for (person, by) in denied {
        let name = format!(
            "never-held-{}",
            person.split('@').next().unwrap_or_default()
        );
        let answer = spawn_answer(&app, person, "meta/ensure_member", &name, "worker").await;
        assert!(
            answer["error"].is_null(),
            "control, the workers ended, {when}: the person denied the spawn by {by} creates \
             a member under a name no agent ever held: {answer}"
        );
        ended_if_seated(&handle, &name).await?;
    }

    let mut wrong = Vec::new();
    for (person, by) in denied {
        for method in SPAWN_METHODS {
            let name = worker_for(person, method);
            let answer = spawn_answer(&app, person, method, &name, "worker").await;
            if !is_the_access_refusal(&answer) {
                let seated = handle
                    .get_member(&AgentIdentity::from(name.as_str()))
                    .await?
                    .is_some();
                wrong.push(format!(
                    "the person denied the spawn by {by} is not refused {method} under the ended \
                     worker's name (a member is seated under it now: {seated}): {}",
                    answer.to_string().chars().take(240).collect::<String>()
                ));
            }
            ended_if_seated(&handle, &name).await?;
        }
    }
    assert!(
        wrong.is_empty(),
        "the workers ended, {when}: a spawn under an ended worker's name is served: {wrong:#?}"
    );
    Ok(())
}

/// [`a_spawn_under_an_ended_workers_name_is_refused_under_a_deny`] with the
/// gateway that seated the workers still running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_under_an_ended_members_name_is_refused_under_a_deny_on_spawn_by_role_or_label()
-> TestResult {
    a_spawn_under_an_ended_workers_name_is_refused_under_a_deny(
        "ended-name-spawn",
        Gateway::StillRunning,
    )
    .await
}

/// [`a_spawn_under_an_ended_workers_name_is_refused_under_a_deny`] after a
/// restart of the gateway on the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_under_an_ended_members_name_is_refused_under_a_deny_on_spawn_by_role_or_label_after_a_restart()
-> TestResult {
    a_spawn_under_an_ended_workers_name_is_refused_under_a_deny(
        "ended-name-spawn-restarted",
        Gateway::Restarted,
    )
    .await
}

/// A deny decides the actions it names. A person who may do everything to
/// every agent and is denied, by the role `worker`, only the view of an
/// agent and the read of its memory creates a member of another role under
/// the name of a worker that ended, with `meta/ensure_member` and with
/// `meta/spawn_helper`: the deny does not name the spawn. With
/// `with_the_reads`, the same person is refused what the deny does name of
/// another worker that ended, as while that worker lived: its records in
/// the memory panel and its status.
async fn a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name(
    mob: &str,
    gateway: Gateway,
    with_the_reads: bool,
) -> TestResult {
    use super::console_side_chat_tests::persistent_gateway_runtime_in;
    use crate::AgentMemoryProvider as _;
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("dave-drives-every-agent", DAVE)
        },
        crate::access::AccessRule {
            actions: vec![
                ACTION_AGENT_VIEW.to_string(),
                ACTION_AGENT_MEMORY_READ.to_string(),
            ],
            roles: vec!["worker".to_string()],
            ..agent_deny("dave-is-denied-reading-the-workers", DAVE)
        },
    ]);
    let state = tempfile::tempdir()?;
    let memory_dir = tempfile::tempdir()?;
    let mut runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    workers_that_ended(&runtime.handle(), &[DAVE]).await?;
    // The worker whose memory and status the person reads.
    seat(&runtime.handle(), "worker", ENDED, vault_tier()).await?;
    let store = crate::memory::SqliteAgentMemoryStore::open(memory_dir.path())?;
    let note = store
        .remember(
            &crate::AgentMemoryConfig::default().realm,
            &crate::identity_first::AgentIdentity::parse(ENDED)?,
            crate::NewAgentMemory {
                title: note_title(ENDED),
                body: "the heron stands in the reeds".to_string(),
                tags: Vec::new(),
            },
        )
        .await?;
    let memory: Arc<dyn crate::memory::capabilities::MemoryPanelStore> = Arc::new(store);
    let mut console = door_console(&runtime, &config, &memory, &note.memory_id)?;
    if with_the_reads {
        for door in [Door::MemoryPanel, Door::MemberStatus] {
            let read = served(&console, door, DAVE).await?;
            assert!(
                read.is_empty(),
                "control, while the worker lives: the person denied reading the workers is \
                 refused, but {read:?}"
            );
        }
    }
    retired(&runtime.handle(), ENDED).await?;
    if let Gateway::Restarted = gateway {
        drop(console);
        runtime = restarted(runtime, state.path(), mob).await?;
        console = door_console(&runtime, &config, &memory, &note.memory_id)?;
    }
    let handle = runtime.handle();
    let when = gateway.words();

    // The limit: the spawn under an ended worker's name is granted.
    for method in SPAWN_METHODS {
        let name = worker_for(DAVE, method);
        let answer = spawn_answer(&console.app, DAVE, method, &name, "lead").await;
        assert!(
            answer["error"].is_null(),
            "the workers ended, {when}: the person denied only the reading of the workers \
             creates a member under an ended worker's name with {method}: {answer}"
        );
        if method == "meta/ensure_member" {
            assert!(
                handle
                    .get_member(&AgentIdentity::from(name.as_str()))
                    .await?
                    .is_some(),
                "the workers ended, {when}: the member is seated under that name"
            );
        }
        ended_if_seated(&handle, &name).await?;
    }
    if !with_the_reads {
        return Ok(());
    }

    // What the deny names stays refused of a worker that ended.
    let mut read = served(&console, Door::MemoryPanel, DAVE).await?;
    read.extend(served(&console, Door::MemberStatus, DAVE).await?);
    assert!(
        read.is_empty(),
        "the worker ended, {when}: the person denied reading the workers is served after the \
         member's end: {read:?}"
    );
    Ok(())
}

/// [`a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name`]
/// with the gateway that seated the workers still running: the spawn is
/// granted, the reads are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_denied_reading_an_ended_members_role_still_spawns_under_its_name() -> TestResult {
    a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name(
        "ended-name-spawn-read-deny",
        Gateway::StillRunning,
        true,
    )
    .await
}

/// [`a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name`]
/// after a restart of the gateway on the same state: the spawn is granted,
/// the reads are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_denied_reading_an_ended_members_role_still_spawns_under_its_name_after_a_restart()
-> TestResult {
    a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name(
        "ended-name-spawn-read-deny-restarted",
        Gateway::Restarted,
        true,
    )
    .await
}

/// The spawn alone of
/// [`a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name`],
/// with the gateway that seated the workers still running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_under_an_ended_members_name_is_granted_to_a_person_denied_only_its_reading()
-> TestResult {
    a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name(
        "ended-name-spawn-granted",
        Gateway::StillRunning,
        false,
    )
    .await
}

/// The spawn alone of
/// [`a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name`],
/// after a restart of the gateway on the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_under_an_ended_members_name_is_granted_to_a_person_denied_only_its_reading_after_a_restart()
-> TestResult {
    a_person_denied_reading_the_workers_spawns_under_an_ended_workers_name(
        "ended-name-spawn-granted-restarted",
        Gateway::Restarted,
        false,
    )
    .await
}

/// A mob made through the runtime's agent mob state, owned by the session
/// `owner`, as a member's own mob tools make one.
async fn mob_owned_by(
    runtime: &MobRuntime,
    mob_id: &str,
    owner: SessionId,
) -> TestResult<MobHandle> {
    let state = runtime
        .agent_mob_mcp_state()
        .ok_or("the runtime carries agent mob tools")?;
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": mob_id, "profiles": profiles() }))?;
    let created = Box::pin(
        state.mob_create_definition_with_owner_bridge_session(definition, owner, true, false),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(Box::pin(state.handle_for(&created))
        .await
        .map_err(|error| error.to_string())?)
}

/// A member of a mob one of the console's members created carries, while
/// it lives, the label of the member that hosts its mob and the label of
/// the mob it sits in. Once it ended neither is known of it: a deny on
/// either label still refuses it, as it did while the member lived, and an
/// allow by either shows nothing of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deny_on_the_label_of_the_mob_a_member_sat_in_still_refuses_it_after_its_end()
-> TestResult {
    const TEAM: &str = "created-mob-team";
    const ERIN: &str = "erin@example.test";
    const FRANK: &str = "frank@example.test";
    const MEMBER: &str = "helper";
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
    let root = tempfile::tempdir()?;
    let runtime = runtime("created-mob", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let owner = primary
        .resolve_bridge_session_id(&AgentIdentity::from("planner"))
        .await
        .ok_or("the planner has a session")?;
    let team = mob_owned_by(&runtime, TEAM, owner).await?;
    seat(&team, "worker", MEMBER, BTreeMap::new()).await?;

    // Bob and Carol may do everything to every agent; Bob is denied the
    // members of the planner's mobs, Carol the members of the team. Erin is
    // granted the members of the planner's mobs, Frank those of the team.
    let every_agent = |id: &str, subject: &str| crate::access::AccessRule {
        agents: vec!["*".to_string()],
        ..agent_rule(id, subject)
    };
    let config = alice_drives_every_agent_and(vec![
        every_agent("bob-drives-every-agent", BOB),
        crate::access::AccessRule {
            match_labels: label("delegate_host_identity", "planner"),
            ..agent_deny("bob-is-denied-the-planners-mobs", BOB)
        },
        every_agent("carol-drives-every-agent", CAROL),
        crate::access::AccessRule {
            match_labels: label("source_mob_id", TEAM),
            ..agent_deny("carol-is-denied-the-team", CAROL)
        },
        crate::access::AccessRule {
            match_labels: label("delegate_host_identity", "planner"),
            ..agent_rule("erin-is-granted-the-planners-mobs", ERIN)
        },
        crate::access::AccessRule {
            match_labels: label("source_mob_id", TEAM),
            ..agent_rule("frank-is-granted-the-team", FRANK)
        },
    ]);
    let sees = |access: &AccessController, subject: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(MEMBER)
    };
    let people = [ALICE, BOB, CAROL, ERIN, FRANK];
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    assert_eq!(
        people.map(|subject| sees(&access, subject)),
        [true, false, false, true, true],
        "control, while the member lives: alice, bob, carol, erin and frank see it"
    );

    retired(&team, MEMBER).await?;
    let access = AccessController::new(config)?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    assert_eq!(
        people.map(|subject| sees(&access, subject)),
        [true, false, false, false, false],
        "the member ended: alice, bob, carol, erin and frank see it"
    );
    Ok(())
}

/// Access rules can match three labels of a member's row that say where the
/// member sits: the mob (`source_mob_id`), the member that hosts that mob
/// (`delegate_host_identity`) and its group (`group`, `Coordinators` for a
/// created mob's members). The console writes them from where it read the
/// member, whatever a spawn registered for it or its roster row holds: a
/// member of the runtime's own mob matches no rule on a created mob's
/// values of these three labels, and a member of the created mob matches
/// every rule on its mob's values of these three labels. The row says
/// nothing of rules on other selectors.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_labels_that_say_where_a_member_sits_are_the_consoles_own_for_access_rules()
-> TestResult {
    const TEAM: &str = "row-keys-team";
    const HOST: &str = "planner";
    const GROUP: &str = "Coordinators";
    // Granted the created mob by one label each.
    const GRANTED: [&str; 3] = [
        "erin@example.test",
        "frank@example.test",
        "grace@example.test",
    ];
    // May drive every agent, denied the created mob by one label each.
    const DENIED: [&str; 3] = [
        "bob@example.test",
        "carol@example.test",
        "dave@example.test",
    ];
    let selectors = [
        ("source_mob_id", TEAM),
        ("delegate_host_identity", HOST),
        ("group", GROUP),
    ];
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
    let labels_naming = |mob: &str, host: &str, group: &str| {
        BTreeMap::from([
            ("source_mob_id".to_string(), mob.to_string()),
            ("delegate_host_identity".to_string(), host.to_string()),
            ("group".to_string(), group.to_string()),
        ])
    };

    let root = tempfile::tempdir()?;
    let runtime = runtime("row-keys-own", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", HOST, BTreeMap::new()).await?;
    let owner = primary
        .resolve_bridge_session_id(&AgentIdentity::from(HOST))
        .await
        .ok_or("the planner has a session")?;
    let team = mob_owned_by(&runtime, TEAM, owner).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::new(ConsoleEventStore::new());
    runtime.install_console_spawn_sink(sink.clone());

    // In the runtime's own mob: a member whose spawn registered the created
    // mob's values, and one whose roster row holds its mob and host (a group
    // on a roster row of the runtime's own mob is the host's own label).
    seat(&primary, "worker", "own-registered", BTreeMap::new()).await?;
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        "own-registered",
        HOST,
        labels_naming(TEAM, HOST, GROUP),
    )
    .await;
    let mut stored = labels_naming(TEAM, HOST, GROUP);
    stored.remove("group");
    seat(&primary, "worker", "own-stored", stored).await?;
    // In the created mob: a plain member, one whose spawn registered other
    // values, and one whose roster row holds other values.
    seat(&team, "worker", "team-plain", BTreeMap::new()).await?;
    seat(&team, "worker", "team-registered", BTreeMap::new()).await?;
    registered_spawn(
        &sink,
        team.mob_id().as_str(),
        "team-registered",
        HOST,
        labels_naming("another-mob", "another-host", "another-group"),
    )
    .await;
    seat(
        &team,
        "worker",
        "team-stored",
        labels_naming("another-mob", "another-host", "another-group"),
    )
    .await?;

    let mut rules = Vec::new();
    for (index, (key, value)) in selectors.into_iter().enumerate() {
        rules.push(crate::access::AccessRule {
            match_labels: label(key, value),
            ..agent_rule(&format!("granted-by-{key}"), GRANTED[index])
        });
        rules.push(crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule(&format!("drives-every-agent-{index}"), DENIED[index])
        });
        rules.push(crate::access::AccessRule {
            match_labels: label(key, value),
            ..agent_deny(&format!("denied-by-{key}"), DENIED[index])
        });
    }
    let access = AccessController::new(alice_drives_every_agent_and(rules))?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let sees = |subject: &str, member: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(member)
    };

    let mut wrong = Vec::new();
    for (member, sits_in_the_team) in [
        ("own-registered", false),
        ("own-stored", false),
        ("team-plain", true),
        ("team-registered", true),
        ("team-stored", true),
    ] {
        let granted = GRANTED.map(|subject| sees(subject, member));
        let denied = DENIED.map(|subject| sees(subject, member));
        if granted != [sits_in_the_team; 3] || denied != [!sits_in_the_team; 3] {
            wrong.push(format!(
                "{member} (in the created mob: {sits_in_the_team}): the three granted by \
                 mob, host and group see it {granted:?}, the three denied by them see it \
                 {denied:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    Ok(())
}

/// A member of the runtime's own mob is read alike wherever the console
/// reads it, whatever its roster row says of a mob or a host: the event
/// stream's projection of the member, which the doors that go by a member's
/// name share their visibility test with, shows it as the snapshot does and
/// names no mob and no host in the attributes access rules are asked with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_of_the_runtimes_own_mob_is_read_alike_by_name_and_by_the_stream() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("row-keys-by-name", root.path()).await?;
    let primary = runtime.handle();
    seat(
        &primary,
        "delegate",
        "labelled",
        BTreeMap::from([
            ("source_mob_id".to_string(), "another-mob".to_string()),
            (
                "delegate_host_identity".to_string(),
                "another-host".to_string(),
            ),
        ]),
    )
    .await?;
    seat(&primary, "delegate", "plain", BTreeMap::new()).await?;
    let policy = crate::console_aggregator::HideImplicitDelegateMembersConsoleVisibilityPolicy;

    let mut wrong = Vec::new();
    for name in ["labelled", "plain"] {
        let member = primary
            .get_member(&AgentIdentity::from(name))
            .await?
            .ok_or("the member is seated")?;
        let projection = sse_member_authorization_projection(
            &primary,
            Some(&runtime),
            &policy,
            name,
            &member.agent_runtime_id,
            member.fence_token,
        )
        .await
        .ok_or("the member's events are attributed")?;
        let labels = &projection.attributes.labels;
        if !projection.visible
            || labels.contains_key("source_mob_id")
            || labels.contains_key("delegate_host_identity")
        {
            wrong.push(format!(
                "{name}: shown {}, labels {labels:?}",
                projection.visible
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    Ok(())
}

/// A caller no deny applies to reads a name the console's records do not
/// know as before: `meta/member_status`, `meta/collect_completed` and a
/// schedule list answer that caller by the method, each with the answer
/// the method gives for such a name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_no_deny_applies_to_reads_an_unknown_name_as_before_this_change() -> TestResult {
    use super::console_side_chat_tests::persistent_gateway_runtime_in;
    use super::console_token_lapse_tests::console_rpc_as;
    const NOBODY: &str = "never-recorded";
    let state = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state.path(), "unknown-name-no-deny", "", "").await?;
    seat(&runtime.handle(), "worker", "alpha", BTreeMap::new()).await?;
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service,
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    // Another person is denied the workers; the caller holds allows only:
    // on every agent, and on every agent action with no agent named (what
    // `meta/collect_completed` asks).
    let config = alice_drives_every_agent_and(vec![
        agent_rule("alice-uses-every-agent-action", ALICE),
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
    ]);
    let app = signed_in_console(&runtime, &config)?;
    let alice = token_of(ALICE);
    let mut wrong = Vec::new();
    for (method, params, answered) in [
        (
            "meta/member_status",
            json!({ "member_id": NOBODY }),
            r#"{"id":"test","jsonrpc":"2.0","result":{"error":null,"is_final":true,"output_preview":null,"peer_connectivity":{"status":"not_applicable"},"progress":{"health":"unknown","in_flight_work":0,"last_progress_at_ms":0,"last_progress_event":"unchanged","run_state":"unknown"},"status":"unknown","tokens_used":0}}"#,
        ),
        (
            "meta/collect_completed",
            json!({}),
            r#"{"id":"test","jsonrpc":"2.0","result":{"completed":[]}}"#,
        ),
        (
            "meta/schedule/list",
            json!({ "identity": NOBODY }),
            r#"{"id":"test","jsonrpc":"2.0","result":{"identity":"never-recorded","other_schedules":[],"schedules":[]}}"#,
        ),
    ] {
        let answer = console_rpc_as(&app, Some(&alice), method, params)
            .await
            .to_string();
        if answer != answered {
            wrong.push(format!("{method} answers {answer}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a caller no deny applies to, on a name the records do not know: {wrong:#?}"
    );
    Ok(())
}

/// Two doors answer for names that are no agents of this console, and a
/// deny that matches neither leaves them shown: an endpoint of the topology
/// that another authority holds, and a configured module in the experience.
/// A person who may view every agent and is denied the agents of a role
/// neither has reads both as a person with allows only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_endpoint_of_another_authority_and_a_module_row_stay_for_a_caller_under_a_deny_that_does_not_match()
-> TestResult {
    use super::console_side_chat_tests::persistent_gateway_runtime_in;
    use crate::topology_control::{
        TopologyControlPolicy, TopologyEndpoint, TopologyNodeSnapshot, TopologySnapshot,
    };
    use tower::ServiceExt;
    // Alice and Bob may do everything to every agent and view the topology
    // of every agent; Bob is denied the auditors.
    let on_every_agent = |id: &str, subject: &str| crate::access::AccessRule {
        actions: vec![
            "agent.*".to_string(),
            crate::access::ACTION_TOPOLOGY_VIEW.to_string(),
        ],
        agents: vec!["*".to_string()],
        ..agent_rule(id, subject)
    };
    let config = AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            on_every_agent("alice-views-every-agent", ALICE),
            on_every_agent("bob-views-every-agent", BOB),
            crate::access::AccessRule {
                effect: crate::access::AccessEffect::Deny,
                actions: vec![
                    "agent.*".to_string(),
                    crate::access::ACTION_TOPOLOGY_VIEW.to_string(),
                ],
                roles: vec!["auditor".to_string()],
                ..agent_rule("bob-is-denied-the-auditors", BOB)
            },
        ],
        ..AccessControlConfig::default()
    };

    // The topology: an endpoint another authority holds.
    let access = AccessController::new(config.clone())?;
    let endpoints_shown = |subject: &str| {
        let mut snapshot = TopologySnapshot {
            authority: "this-gateway".to_string(),
            revision: 1,
            policy: TopologyControlPolicy::default(),
            nodes: vec![TopologyNodeSnapshot {
                endpoint: TopologyEndpoint {
                    authority: Some("another-gateway".to_string()),
                    identity: "far-worker".to_string(),
                },
                role: "worker".to_string(),
                labels: BTreeMap::new(),
                affordances: None,
            }],
            edges: Vec::new(),
        };
        snapshot.retain_visible_to(
            &access.view_for_caller(Some(subject), Some(&format!("person:{subject}"))),
        );
        snapshot.nodes.len()
    };
    assert_eq!(
        [ALICE, BOB].map(endpoints_shown),
        [1, 1],
        "the endpoint of another authority is shown to the person with allows only and to the \
         person under a deny that does not match it"
    );

    // The experience of a console with a configured module.
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "module-row-stays", "", "").await?;
    let app = signed_in_console(&runtime, &config)?;
    let mut modules = Vec::new();
    for person in [ALICE, BOB] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/console/experience")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", token_of(person)),
                    )
                    .body(axum::body::Body::empty())
                    .expect("experience request"),
            )
            .await
            .expect("experience response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 22)
            .await
            .expect("experience body");
        let experience: Value = serde_json::from_slice(&body)?;
        modules.push(
            experience
                .pointer("/health_overview/live_snapshot/loaded_modules")
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
    assert!(
        modules[0]
            .as_array()
            .is_some_and(|listed| listed.contains(&json!("router"))),
        "the experience names the configured module to the person with allows only: {}",
        modules[0]
    );
    assert_eq!(
        modules[1], modules[0],
        "and to the person under a deny that does not match it"
    );
    Ok(())
}

/// The owner of an ended private member is answered alike by the memory
/// panel and by the status door. Once the member has ended the console
/// knows its name and who it was private to, not its role: a deny by role
/// that applies to its owner cannot be ruled out, so neither door serves
/// her, though her other rule allows every agent; an owner no deny applies
/// to is served by both. The panel decides the ended member by the entry
/// the access records hold, as every door does, and not as an agent that
/// has no role.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_memory_panel_and_the_status_door_answer_the_owner_of_an_ended_private_member_alike()
-> TestResult {
    use super::console_side_chat_tests::persistent_gateway_runtime_in;
    use super::console_token_lapse_tests::console_rpc_as;
    use crate::AgentMemoryProvider as _;

    // The owner a deny by role applies to, then the owner no rule refuses.
    for (mob, owner, served_by_each) in [
        ("ended-private-owner-denied-by-role", BOB, false),
        ("ended-private-owner-unrefused", ALICE, true),
    ] {
        let state = tempfile::tempdir()?;
        let memory_dir = tempfile::tempdir()?;
        let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
        seat(&runtime.handle(), "lead", "planner", BTreeMap::new()).await?;
        let store = crate::memory::SqliteAgentMemoryStore::open(memory_dir.path())?;
        let note = store
            .remember(
                &crate::AgentMemoryConfig::default().realm,
                &crate::identity_first::AgentIdentity::parse(ENDED)?,
                crate::NewAgentMemory {
                    title: note_title(ENDED),
                    body: "the heron stands in the reeds".to_string(),
                    tags: Vec::new(),
                },
            )
            .await?;
        let memory: Arc<dyn crate::memory::capabilities::MemoryPanelStore> = Arc::new(store);
        let config = rules_of_the_door_rows(DeniedBy::Role);
        let console = door_console(&runtime, &config, &memory, &note.memory_id)?;
        // The member is private to the owner: its roster row carries the
        // owner's key, as a host seats such a member.
        let owner_key = console_rpc_as(
            &console.app,
            Some(&token_of(owner)),
            "meta/console/whoami",
            json!({}),
        )
        .await["result"]["caller_scope"]
            .as_str()
            .expect("the owner's caller scope")
            .to_string();
        seat(
            &runtime.handle(),
            "worker",
            ENDED,
            BTreeMap::from([(crate::access::PRIVATE_TO_LABEL.to_string(), owner_key)]),
        )
        .await?;
        retired(&runtime.handle(), ENDED).await?;
        assert!(
            runtime
                .handle()
                .get_member(&AgentIdentity::from(ENDED))
                .await?
                .is_none(),
            "the member has ended"
        );
        let by_the_panel = served(&console, Door::MemoryPanel, owner).await?;
        let by_the_status_door = served(&console, Door::MemberStatus, owner).await?;
        assert_eq!(
            (!by_the_panel.is_empty(), !by_the_status_door.is_empty()),
            (served_by_each, served_by_each),
            "{owner}, the owner of the ended private member: the panel serves {by_the_panel:?}, \
             the status door {by_the_status_door:?}"
        );
    }
    Ok(())
}
