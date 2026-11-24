//! The console's send and its recovery for a mob the host serves beside its
//! launch mob, asked through the console's doors: the request door
//! (`meta/console/send`, `meta/console/recover_send`) and the two routes
//! (`POST /console/send`, `POST /console/send/recover`).
//!
//! The fixture is two runtimes side by side, each with an identity of one
//! name, on a console whose resolver serves the second as a mob beside the
//! launch mob and whose frame log holds the registration of the launch
//! mob's runtime and of the second mob's console events, as a host
//! registers them. The console requires a sign-in and enforces access
//! rules: one administrator, a person whose rule gives the send in the
//! launch mob, a person whose rule gives the send in the second mob, a
//! person whose rule gives the view of the second mob's agents and no
//! send, and a person no rule names.

use super::*;

/// The identity each of the two mobs has.
const SHARED_NAME: &str = "alice";
/// The runtime key the launch mob's runtime is registered under.
const LAUNCH_REGISTRATION: &str = "runtime-a";
/// The person whose rule gives the send in the launch mob: it lists no mob.
const SENDS_IN_THE_LAUNCH_MOB: &str = DOOR_ROWS_OPERATOR;
/// The person whose rule gives the send in the second mob.
const SENDS_IN_THE_SECOND_MOB: &str = DOOR_ROWS_OF_THE_SECOND_MOB;
/// The person whose rule gives the view of the second mob's agents and no
/// send: the person may address that mob and may not send in it.
const SEES_THE_SECOND_MOB: &str = DOOR_ROWS_ALLOWED_AND_DENIED;

/// The second identity of a mob whose [`SHARED_NAME`] carries a label: it
/// carries none.
const UNLABELLED_NAME: &str = "bob";

/// The label a rule of the rows keeps a member from the sends of
/// [`SENDS_IN_THE_SECOND_MOB`] by, in the second mob.
const KEPT_FROM_SENDS: (&str, &str) = ("door-send-rows", "kept-from-sends");

/// A runtime over one mob whose id starts with `tag`, with an identity
/// plane of its own whose one identity is [`SHARED_NAME`], and the state
/// directory the runtime keeps.
async fn runtime_with_an_identity(tag: &str) -> (tempfile::TempDir, crate::UnifiedRuntime) {
    runtime_with_identities(tag, vec![(SHARED_NAME, std::collections::BTreeMap::new())]).await
}

/// [`runtime_with_an_identity`] whose identity carries `labels`, beside a
/// second identity, [`UNLABELLED_NAME`], that carries none.
async fn runtime_with_a_labelled_identity(
    tag: &str,
    labels: std::collections::BTreeMap<String, String>,
) -> (tempfile::TempDir, crate::UnifiedRuntime) {
    runtime_with_identities(
        tag,
        vec![
            (SHARED_NAME, labels),
            (UNLABELLED_NAME, std::collections::BTreeMap::new()),
        ],
    )
    .await
}

/// A runtime over one mob whose id starts with `tag`, with an identity
/// plane of its own whose identities are `identities` (each name with its
/// labels), and the state directory the runtime keeps.
async fn runtime_with_identities(
    tag: &str,
    identities: Vec<(&str, std::collections::BTreeMap<String, String>)>,
) -> (tempfile::TempDir, crate::UnifiedRuntime) {
    use crate::identity_first::{
        AgentAddressability, AgentIdentity, DurableAgentSpec, MutableRosterProvider,
    };
    let state_dir = tempfile::tempdir_in(std::env::current_dir().expect("the working directory"))
        .expect("a state directory");
    let roster = identities
        .into_iter()
        .map(|(name, labels)| DurableAgentSpec {
            identity: AgentIdentity::parse(name).expect("identity"),
            profile: "agent".into(),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels,
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
            backend: None,
            binding: None,
            placement: None,
        })
        .collect::<Vec<_>>();
    let mob_id = format!("console-door-{tag}-{}", uuid::Uuid::new_v4().simple());
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.agent]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"autonomous_host\"\n\n\
         [profiles.agent.tools]\ncomms = true\n"
    ))
    .expect("mob definition");
    let runtime = Box::pin(
        crate::UnifiedRuntime::builder()
            .definition(definition)
            .persistent_state(state_dir.path())
            .comms(true)
            .roster_provider(Arc::new(MutableRosterProvider::new(roster)))
            .identity_runtime_instance_id(format!("{mob_id}-instance"))
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .expect("runtime builds");
    (state_dir, runtime)
}

/// The state of the rows' console over `launch`'s mob: [`door_rows_state`]
/// with the rows' own access rules about the send, the frame log
/// `aggregator` and the host's console events.
fn send_rows_state(
    launch: &crate::UnifiedRuntime,
    second_mob: &str,
    aggregator: MetaConsoleAggregator,
) -> ConsoleJsonState {
    let sends = vec![
        crate::access::ACTION_AGENT_SEND.to_string(),
        crate::access::ACTION_AGENT_VIEW.to_string(),
    ];
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "sends-in-the-launch-mob".to_string(),
                subjects: vec![SENDS_IN_THE_LAUNCH_MOB.to_string()],
                actions: sends.clone(),
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "sends-in-the-second-mob".to_string(),
                subjects: vec![SENDS_IN_THE_SECOND_MOB.to_string()],
                actions: sends,
                mobs: vec![second_mob.to_string()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "sees-the-second-mob".to_string(),
                subjects: vec![SEES_THE_SECOND_MOB.to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec![second_mob.to_string()],
                ..crate::access::AccessRule::default()
            },
            // A rule by label, of the second mob: it is decided on the
            // attributes recorded for that mob's agents.
            crate::access::AccessRule {
                id: "kept-from-sends-in-the-second-mob".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec![SENDS_IN_THE_SECOND_MOB.to_string()],
                actions: vec![crate::access::ACTION_AGENT_SEND.to_string()],
                match_labels: std::collections::BTreeMap::from([(
                    KEPT_FROM_SENDS.0.to_string(),
                    KEPT_FROM_SENDS.1.to_string(),
                )]),
                mobs: vec![second_mob.to_string()],
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    ConsoleJsonState {
        access: Some(controller),
        console_aggregator: Some(aggregator),
        console_events: Some(launch.console_events()),
        ..door_rows_state(launch, second_mob)
    }
}

/// A `POST` of `body` to the console route `uri`.
fn post_route(uri: &str, body: &Value) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("POST request")
}

/// The params of a send of the key `key` to the member `identity` of the
/// mob `mob_id` (none: a send that names no mob).
fn send_params(mob_id: Option<&str>, identity: &str, key: &str) -> Value {
    let mut params = recover_params(mob_id, identity, key);
    params["content"] = json!("hello");
    params
}

/// The params of the recovery of the send [`send_params`] makes.
fn recover_params(mob_id: Option<&str>, identity: &str, key: &str) -> Value {
    let mut params = naming_mob(mob_id);
    params["identity"] = json!(identity);
    params["origin"] = json!("console");
    params["idempotency_key"] = json!(key);
    params
}

/// The door a send and its recovery are asked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Door {
    /// The console's request door: the two methods.
    RequestDoor,
    /// The two console routes.
    Route,
}

impl Door {
    const BOTH: [Self; 2] = [Self::RequestDoor, Self::Route];

    /// The door as [`is_the_mob_ids_refusal`] names it.
    fn name(self) -> &'static str {
        match self {
            Self::RequestDoor => "console RPC",
            Self::Route => "POST /console/send",
        }
    }

    /// The send of `params` on this door.
    fn send(self, params: &Value) -> axum::http::Request<axum::body::Body> {
        match self {
            Self::RequestDoor => rpc_request("meta/console/send", params),
            Self::Route => post_route("/console/send", params),
        }
    }

    /// The recovery of `params` on this door.
    fn recover(self, params: &Value) -> axum::http::Request<axum::body::Body> {
        match self {
            Self::RequestDoor => rpc_request("meta/console/recover_send", params),
            Self::Route => post_route("/console/send/recover", params),
        }
    }

    /// The id of the send's frame, when `answer` is an acceptance (of a
    /// send, or of a recovery, which answers the acceptance as it is now).
    fn accepted(self, answer: &DoorAnswer) -> Option<String> {
        let body = body_of(answer);
        let accepted = match self {
            Self::RequestDoor => body["result"].clone(),
            Self::Route if answer.0 == StatusCode::OK => body,
            Self::Route => Value::Null,
        };
        accepted["input_frame_id"].as_str().map(str::to_string)
    }

    /// Whether `answer` is a recovery's answer for a key the caller has no
    /// send under.
    fn found_no_send(self, answer: &DoorAnswer) -> bool {
        let body = body_of(answer);
        match self {
            Self::RequestDoor => {
                body["error"]["code"] == json!(-32001)
                    && body["error"]["data"]["kind"] == json!("send_not_found")
            }
            Self::Route => {
                answer.0 == StatusCode::NOT_FOUND && body["error"] == json!("send_not_found")
            }
        }
    }

    /// What `answer` refused with: the error's code on the request door,
    /// the status and the error's kind on a route.
    fn refusal(self, answer: &DoorAnswer) -> String {
        let body = body_of(answer);
        match self {
            Self::RequestDoor => body["error"]["code"].to_string(),
            Self::Route => format!("{} {}", answer.0, body["error"]),
        }
    }

    /// Whether `answer` is the refusal of a governed send to a member of
    /// the mob `mob`, a mob the host serves beside its launch mob: the
    /// typed refusal of a mob id that says the host has the mob, and that
    /// a governed send serves the launch mob only.
    fn is_the_governed_refusal(self, answer: &DoorAnswer, mob: &str) -> bool {
        let body = body_of(answer);
        let (status, typed, message) = match self {
            Self::RequestDoor => (
                StatusCode::OK,
                body["error"]["code"] == json!(-32602)
                    && body["error"]["data"]["kind"] == json!("foreign_mob_target")
                    && body["error"]["data"]["requested_mob_id"] == json!(mob)
                    && body["error"]["data"]["hosted"] == json!(true),
                &body["error"]["message"],
            ),
            Self::Route => (
                StatusCode::NOT_FOUND,
                body["error"] == json!("foreign_mob_target")
                    && body["requested_mob_id"] == json!(mob)
                    && body["hosted"] == json!(true),
                &body["message"],
            ),
        };
        answer.0 == status
            && typed
            && message
                .as_str()
                .is_some_and(|message| message.contains("serves the launch mob only"))
    }
}

/// The send frames a member holds in the console's frame log, as the rows
/// read them: each frame's id and the mob it names; or why they could not
/// be read.
type HeldSends = Result<Vec<(String, Option<String>)>, String>;

/// The host of the rows: the launch mob and the second mob, each with a
/// member [`SHARED_NAME`], the console's frame log, the console and the
/// resolver its router carries.
struct TwoMobsOfOneName {
    launch: crate::UnifiedRuntime,
    second: crate::UnifiedRuntime,
    aggregator: MetaConsoleAggregator,
    app: Router,
    resolver: crate::unified_runtime::MobResolver,
    _state_dirs: Vec<tempfile::TempDir>,
}

impl TwoMobsOfOneName {
    /// Two mobs that each have an identity plane.
    async fn start() -> Self {
        Self::start_with(None).await
    }

    /// [`Self::start`] on a host that installed `ingress` as its governed
    /// ingress of Console sends.
    async fn start_with(
        ingress: Option<Arc<dyn crate::console_governed_ingress::ConsoleGovernedIngress>>,
    ) -> Self {
        let (launch_dir, launch) = runtime_with_an_identity("launch").await;
        let (second_dir, second) = runtime_with_an_identity("second").await;
        Self::over(launch, second, vec![launch_dir, second_dir], ingress)
    }

    /// The console over `launch`'s mob that serves `second`'s mob beside
    /// it. The frame log was told its default mob and holds the
    /// registration of the launch mob's runtime, with its identity
    /// runtime, and of the second mob's console events.
    fn over(
        launch: crate::UnifiedRuntime,
        second: crate::UnifiedRuntime,
        state_dirs: Vec<tempfile::TempDir>,
        ingress: Option<Arc<dyn crate::console_governed_ingress::ConsoleGovernedIngress>>,
    ) -> Self {
        let aggregator = MetaConsoleAggregator::new(Arc::new(
            crate::console_aggregator::InMemoryConsoleLogStore::new(),
        ));
        aggregator.tell_default_mob(crate::member_key::DefaultMob::new(
            launch.mob_handle().mob_id().clone(),
        ));
        aggregator.register_runtime_handles_with_policy(
            LAUNCH_REGISTRATION,
            "",
            launch.mob_runtime().clone(),
            launch.identity_runtime_ref().cloned(),
            launch.console_events(),
            Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
        );
        aggregator
            .register_mob_console_events(
                second.mob_handle().mob_id(),
                second.mob_runtime().clone(),
                second.console_events(),
                Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
            )
            .expect("the second mob's console events register");
        let state = send_rows_state(&launch, second.mob_id().as_str(), aggregator.clone());
        if let Some(ingress) = ingress {
            state.governed_ingress.install(ingress);
        }
        let (app, resolver) = console_of_two_mobs_over(state, &launch, &second);
        Self {
            launch,
            second,
            aggregator,
            app,
            resolver,
            _state_dirs: state_dirs,
        }
    }

    /// The send frames the member [`SHARED_NAME`] of the mob `mob` (none:
    /// the launch mob) holds in the console's frame log: each frame's id
    /// and the mob it names.
    async fn sends_of(&self, mob: Option<&str>) -> HeldSends {
        self.aggregator
            .query_mob_timeline_windowed_stored_for(
                ConsoleTimelineWindowQuery {
                    identity: Some(SHARED_NAME.to_string()),
                    mob_id: mob.map(meerkat_mob::MobId::from),
                    limit: 500,
                    ..Default::default()
                },
                None,
            )
            .await
            .map(|page| {
                page.frames
                    .into_iter()
                    .filter(|frame| frame.kind == "user_input")
                    .map(|frame| (frame.id, frame.mob_id.map(|mob| mob.to_string())))
                    .collect()
            })
            .map_err(|error| format!("{error:?}"))
    }

    /// The status of the send frame `frame` of the member [`SHARED_NAME`]
    /// of the mob `mob` (none: the launch mob), read until it is
    /// `delivered` or twenty seconds passed: the last status read, or why
    /// none was read.
    async fn settled_status(&self, mob: Option<&str>, frame: Option<&String>) -> String {
        let Some(frame) = frame else {
            return "no send was accepted".to_string();
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let read = self
                .aggregator
                .query_mob_timeline_windowed_stored_for(
                    ConsoleTimelineWindowQuery {
                        identity: Some(SHARED_NAME.to_string()),
                        mob_id: mob.map(meerkat_mob::MobId::from),
                        limit: 500,
                        ..Default::default()
                    },
                    None,
                )
                .await;
            let status = match read {
                Ok(page) => page
                    .frames
                    .into_iter()
                    .find(|held| &held.id == frame)
                    .map_or_else(
                        || "the member does not hold the frame".to_string(),
                        |held| held.status.as_str().to_string(),
                    ),
                Err(error) => format!("{error:?}"),
            };
            if was_delivered(&status) || tokio::time::Instant::now() >= deadline {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// The frames the member [`SHARED_NAME`] of the mob `mob` (none: the
    /// launch mob) holds in the console's frame log, as stored.
    async fn frames_of(&self, mob: Option<&str>) -> Result<Vec<ConsoleFrame>, String> {
        self.aggregator
            .query_mob_timeline_windowed_stored_for(
                ConsoleTimelineWindowQuery {
                    identity: Some(SHARED_NAME.to_string()),
                    mob_id: mob.map(meerkat_mob::MobId::from),
                    limit: 500,
                    ..Default::default()
                },
                None,
            )
            .await
            .map(|page| page.frames)
            .map_err(|error| format!("{error:?}"))
    }

    /// The one send frame the member [`SHARED_NAME`] of the mob `mob`
    /// holds, read until its status is no longer `accepted` or twenty
    /// seconds passed: its id and its status word; or no id and what was
    /// found instead of one send frame.
    async fn one_send_settled(&self, mob: Option<&str>) -> (Option<String>, String) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let sends: Result<Vec<ConsoleFrame>, String> =
                self.frames_of(mob).await.map(|frames| {
                    frames
                        .into_iter()
                        .filter(|frame| frame.kind == "user_input")
                        .collect()
                });
            let (frame, status) = match sends.as_deref() {
                Ok([send]) => (Some(send.id.clone()), send.status.as_str().to_string()),
                Ok(sends) => (None, format!("{} send frames", sends.len())),
                Err(error) => (None, error.clone()),
            };
            if (frame.is_some() && status != "accepted") || tokio::time::Instant::now() >= deadline
            {
                return (frame, status);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Whether the member [`SHARED_NAME`] of the mob `mob` holds the
    /// terminal of a delivered steer for the send frame `frame`, under that
    /// mob: a completed `interaction_complete` frame the console wrote
    /// itself, whose parent and cause are the send's frame, with the reason
    /// `steer_delivered` and the handling mode `steer`.
    async fn holds_a_steer_terminal(&self, mob: Option<&str>, frame: Option<&String>) -> bool {
        let Some(frame) = frame else {
            return false;
        };
        self.frames_of(mob).await.is_ok_and(|frames| {
            frames.iter().any(|held| {
                held.kind == "interaction_complete"
                    && held.status.as_str() == "completed"
                    && held.parent_frame_id.as_ref() == Some(frame)
                    && held.caused_by_frame_id.as_ref() == Some(frame)
                    && held.source.kind == ConsoleFrameSourceKind::Synthetic
                    && held.payload["reason"] == json!("steer_delivered")
                    && held.payload["handling_mode"] == json!("steer")
                    && held.mob_id.as_ref().map(meerkat_mob::MobId::as_str) == mob
            })
        })
    }

    async fn shut_down(self) {
        drop(self.app);
        self.launch.shutdown().await;
        self.second.shutdown().await;
    }
}

/// Whether the member's send frames `held` hold the frame `frame`.
fn holds(held: &HeldSends, frame: Option<&String>) -> bool {
    match (held, frame) {
        (Ok(held), Some(frame)) => held.iter().any(|(id, _)| id == frame),
        _ => false,
    }
}

/// Whether the member's send frames `held` were read and every one of
/// them names the mob `mob` (none: no mob, as a frame of the launch mob's
/// member does).
fn all_name(held: &HeldSends, mob: Option<&str>) -> bool {
    held.as_ref()
        .is_ok_and(|held| held.iter().all(|(_, named)| named.as_deref() == mob))
}

/// A send that names a mob the host serves beside its launch mob is that
/// mob's send, and its recovery acts on that same send. An administrator
/// sends to the member of the second mob that shares its name with a
/// member of the launch mob. The send is accepted; its frame names the
/// second mob and stands among the frames of that mob's member, and the
/// launch mob's member of the name holds no send. The recovery of the key
/// for the second mob answers that send, and the recovery of the key with
/// no mob finds none. The same key sent with no mob is another send, the
/// launch mob's member's own: each mob's member then holds its own send
/// and not the other's, and each recovery answers its own mob's.
async fn a_hosted_mobs_send_and_its_recovery_are_that_mobs_own(door: Door) {
    const KEY: &str = "door-send-of-one-name";
    let host = TwoMobsOfOneName::start().await;
    let second_mob = host.second.mob_id();
    let admin = DOOR_ROWS_ADMIN;
    let send = |mob: Option<&str>| door.send(&send_params(mob, SHARED_NAME, KEY));
    let recover = |mob: Option<&str>| door.recover(&recover_params(mob, SHARED_NAME, KEY));

    let sent = ask_as(&host.app, send(Some(second_mob.as_str())), admin).await;
    let of_the_second = host.sends_of(Some(second_mob.as_str())).await;
    let of_the_launch = host.sends_of(None).await;
    let recovered = ask_as(&host.app, recover(Some(second_mob.as_str())), admin).await;
    let recovered_with_no_mob = ask_as(&host.app, recover(None), admin).await;
    // The same key for the launch mob's member of the name.
    let sent_with_no_mob = ask_as(&host.app, send(None), admin).await;
    let of_the_second_then = host.sends_of(Some(second_mob.as_str())).await;
    let of_the_launch_then = host.sends_of(None).await;
    let recovered_then = ask_as(&host.app, recover(Some(second_mob.as_str())), admin).await;
    let recovered_with_no_mob_then = ask_as(&host.app, recover(None), admin).await;
    host.shut_down().await;

    let frame = door.accepted(&sent);
    let launch_frame = door.accepted(&sent_with_no_mob);
    let mut wrong = Vec::new();
    if frame.is_none() {
        wrong.push(format!(
            "the send that names the second mob is not accepted: {} {}",
            sent.0, sent.2
        ));
    }
    if !holds(&of_the_second, frame.as_ref())
        || !all_name(&of_the_second, Some(second_mob.as_str()))
    {
        wrong.push(format!(
            "the second mob's member does not hold the send under its mob: {of_the_second:?}"
        ));
    }
    if of_the_launch != Ok(Vec::new()) {
        wrong.push(format!(
            "the launch mob's member of the name holds a send: {of_the_launch:?}"
        ));
    }
    if frame.is_none() || door.accepted(&recovered) != frame {
        wrong.push(format!(
            "the recovery that names the second mob does not answer that send: {} {}",
            recovered.0, recovered.2
        ));
    }
    if !door.found_no_send(&recovered_with_no_mob) {
        wrong.push(format!(
            "the recovery of the key with no mob finds a send: {} {}",
            recovered_with_no_mob.0, recovered_with_no_mob.2
        ));
    }
    if launch_frame.is_none() || launch_frame == frame {
        wrong.push(format!(
            "the same key with no mob is not a send of its own: {} {}",
            sent_with_no_mob.0, sent_with_no_mob.2
        ));
    }
    if !holds(&of_the_launch_then, launch_frame.as_ref())
        || holds(&of_the_launch_then, frame.as_ref())
        || !all_name(&of_the_launch_then, None)
    {
        wrong.push(format!(
            "the launch mob's member does not hold its own send alone: {of_the_launch_then:?}"
        ));
    }
    if !holds(&of_the_second_then, frame.as_ref())
        || holds(&of_the_second_then, launch_frame.as_ref())
        || !all_name(&of_the_second_then, Some(second_mob.as_str()))
    {
        wrong.push(format!(
            "the second mob's member does not hold its own send alone: {of_the_second_then:?}"
        ));
    }
    if frame.is_none() || door.accepted(&recovered_then) != frame {
        wrong.push(format!(
            "after the launch mob's send the recovery for the second mob does not answer the \
             second mob's: {} {}",
            recovered_then.0, recovered_then.2
        ));
    }
    if launch_frame.is_none() || door.accepted(&recovered_with_no_mob_then) != launch_frame {
        wrong.push(format!(
            "after the launch mob's send the recovery with no mob does not answer the launch \
             mob's: {} {}",
            recovered_with_no_mob_then.0, recovered_with_no_mob_then.2
        ));
    }
    assert!(
        wrong.is_empty(),
        "a send for a hosted mob and its recovery, on {door:?}:\n{}",
        wrong.join("\n")
    );
}

#[tokio::test]
async fn a_hosted_mobs_send_and_its_recovery_are_that_mobs_own_on_the_request_door() {
    a_hosted_mobs_send_and_its_recovery_are_that_mobs_own(Door::RequestDoor).await;
}

#[tokio::test]
async fn a_hosted_mobs_send_and_its_recovery_are_that_mobs_own_on_the_routes() {
    a_hosted_mobs_send_and_its_recovery_are_that_mobs_own(Door::Route).await;
}

/// The address check of a send and of its recovery. Under enforced access
/// a caller no rule lets address the second mob (a person no rule names,
/// and a person whose rule gives the send and lists no mob, so that it is
/// the launch mob's) is answered the mob id's refusal for a send and for a
/// recovery that name it, on the request door and on the routes: exactly
/// the answer for an id no mob has, apart from the id, with no word of
/// another mob. The resolver is asked nothing about the second mob, and
/// no member holds a send. The person whose rule gives the send in the
/// launch mob does send to the launch mob's member.
#[tokio::test]
async fn a_caller_who_may_not_address_a_hosted_mob_is_refused_its_send_and_its_recovery() {
    let host = TwoMobsOfOneName::start().await;
    let launch_mob = host.launch.mob_id();
    let second_mob = host.second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);

    let mut wrong = Vec::new();
    for (who, email) in [
        ("a person no rule names", DOOR_ROWS_WITHOUT_RULES),
        (
            "a person whose send rule lists no mob",
            SENDS_IN_THE_LAUNCH_MOB,
        ),
    ] {
        for door in Door::BOTH {
            for what in ["the send", "the recovery"] {
                let naming = |mob: &str| {
                    let key = "door-send-not-addressed";
                    if what == "the send" {
                        door.send(&send_params(Some(mob), SHARED_NAME, key))
                    } else {
                        door.recover(&recover_params(Some(mob), SHARED_NAME, key))
                    }
                };
                let for_the_second = ask_as(&host.app, naming(&second_mob), email).await;
                let for_the_absent = ask_as(&host.app, naming(&absent_mob), email).await;
                if !is_the_mob_ids_refusal(door.name(), &for_the_second, &second_mob) {
                    wrong.push(format!(
                        "{who}, {door:?}, {what}: is not answered the mob id's refusal: \
                         {for_the_second:?}"
                    ));
                }
                if apart_from_the_id(&for_the_second, &second_mob)
                    != apart_from_the_id(&for_the_absent, &absent_mob)
                {
                    wrong.push(format!(
                        "{who}, {door:?}, {what}: is answered differently for the second mob and \
                         for an id no mob has: {for_the_second:?} / {for_the_absent:?}"
                    ));
                }
                if for_the_second.2.contains(&launch_mob) {
                    wrong.push(format!(
                        "{who}, {door:?}, {what}: the answer names a mob the request did not \
                         name: {for_the_second:?}"
                    ));
                }
            }
        }
    }
    let asked_about_the_second_mob = asked_about(&host.resolver, &second_mob);
    let of_the_second = host.sends_of(Some(second_mob.as_str())).await;
    let of_the_launch = host.sends_of(None).await;
    // The fixture: the person whose rule lists no mob holds the send in the
    // launch mob.
    let mut sent_in_the_launch_mob = Vec::new();
    for door in Door::BOTH {
        let key = format!("door-send-in-the-launch-mob-{door:?}");
        let sent = ask_as(
            &host.app,
            door.send(&send_params(None, SHARED_NAME, &key)),
            SENDS_IN_THE_LAUNCH_MOB,
        )
        .await;
        sent_in_the_launch_mob.push((door, door.accepted(&sent), sent));
    }
    host.shut_down().await;

    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about a mob the caller may not address: \
             {asked_about_the_second_mob} times"
        ));
    }
    if of_the_second != Ok(Vec::new()) || of_the_launch != Ok(Vec::new()) {
        wrong.push(format!(
            "a refused send left a frame: {of_the_second:?} / {of_the_launch:?}"
        ));
    }
    for (door, accepted, sent) in sent_in_the_launch_mob {
        if accepted.is_none() {
            wrong.push(format!(
                "the fixture, {door:?}: the person whose rule gives the send in the launch mob \
                 is refused there: {} {}",
                sent.0, sent.2
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a caller who may not address a hosted mob, its send and its recovery:\n{}",
        wrong.join("\n")
    );
}

/// The send rule is asked in the mob the send is for. The person whose
/// rule gives the send in the second mob sends to that mob's member and
/// recovers that send, and is refused by the access rules a send to the
/// launch mob's member of the same name. The person who may address the
/// second mob and holds no send in it is refused its send and its recovery
/// by the access rules, as that refusal is given in the launch mob, and
/// not by the mob id's refusal. No refused send leaves a frame.
async fn the_send_rule_is_asked_in_the_mob_the_send_is_for(door: Door) {
    const KEY: &str = "door-send-by-its-rule";
    let host = TwoMobsOfOneName::start().await;
    let second_mob = host.second.mob_id();
    let to = |mob: Option<&str>, key: &str| door.send(&send_params(mob, SHARED_NAME, key));
    let of = |key: &str| door.recover(&recover_params(Some(second_mob.as_str()), SHARED_NAME, key));

    let in_the_second = ask_as(
        &host.app,
        to(Some(second_mob.as_str()), KEY),
        SENDS_IN_THE_SECOND_MOB,
    )
    .await;
    let in_the_launch = ask_as(&host.app, to(None, KEY), SENDS_IN_THE_SECOND_MOB).await;
    let without_the_send = ask_as(
        &host.app,
        to(Some(second_mob.as_str()), "door-send-without-its-rule"),
        SEES_THE_SECOND_MOB,
    )
    .await;
    let recovered = ask_as(&host.app, of(KEY), SENDS_IN_THE_SECOND_MOB).await;
    let recovered_without_the_send = ask_as(&host.app, of(KEY), SEES_THE_SECOND_MOB).await;
    let of_the_second = host.sends_of(Some(second_mob.as_str())).await;
    let of_the_launch = host.sends_of(None).await;
    host.shut_down().await;

    let frame = door.accepted(&in_the_second);
    let mut wrong = Vec::new();
    if frame.is_none() {
        wrong.push(format!(
            "the person whose rule gives the send in the second mob is not accepted there: {} {}",
            in_the_second.0, in_the_second.2
        ));
    }
    if door.accepted(&in_the_launch).is_some() {
        wrong.push(format!(
            "that person's send to the launch mob's member is accepted: {}",
            in_the_launch.2
        ));
    }
    for (what, answer) in [
        ("send", &without_the_send),
        ("recovery", &recovered_without_the_send),
    ] {
        if door.accepted(answer).is_some()
            || is_the_mob_ids_refusal(door.name(), answer, &second_mob)
            || door.refusal(answer) != door.refusal(&in_the_launch)
        {
            wrong.push(format!(
                "the {what} of a person who may address the second mob and holds no send in it \
                 is not refused by the access rules ({}): {} {}",
                door.refusal(&in_the_launch),
                answer.0,
                answer.2
            ));
        }
    }
    if frame.is_none() || door.accepted(&recovered) != frame {
        wrong.push(format!(
            "the sender's recovery does not answer the send: {} {}",
            recovered.0, recovered.2
        ));
    }
    let one_send = of_the_second.as_ref().is_ok_and(|held| held.len() == 1);
    if !holds(&of_the_second, frame.as_ref()) || !one_send {
        wrong.push(format!(
            "the second mob's member does not hold the one accepted send: {of_the_second:?}"
        ));
    }
    if of_the_launch != Ok(Vec::new()) {
        wrong.push(format!(
            "the launch mob's member holds a send: {of_the_launch:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "the send rule of a hosted mob, on {door:?}:\n{}",
        wrong.join("\n")
    );
}

#[tokio::test]
async fn the_send_rule_is_asked_in_the_mob_the_send_is_for_on_the_request_door() {
    the_send_rule_is_asked_in_the_mob_the_send_is_for(Door::RequestDoor).await;
}

#[tokio::test]
async fn the_send_rule_is_asked_in_the_mob_the_send_is_for_on_the_routes() {
    the_send_rule_is_asked_in_the_mob_the_send_is_for(Door::Route).await;
}

/// A host's governed ingress that counts what it is asked, governs no
/// identity and binds nothing.
#[derive(Default)]
struct CountingIngress {
    asked: std::sync::atomic::AtomicUsize,
}

impl CountingIngress {
    fn asked(&self) -> usize {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl crate::console_governed_ingress::ConsoleGovernedIngress for CountingIngress {
    fn governs(&self, _identity: &crate::identity_first::AgentIdentity) -> bool {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        false
    }

    fn binder_for(
        &self,
        _identity: &crate::identity_first::AgentIdentity,
        _human: &crate::runtime::AuthenticatedConsoleHuman,
    ) -> Result<Arc<dyn crate::identity_first::GovernedInputBinder>, String> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("this fixture binds nothing".to_string())
    }
}

/// On a host that installed a governed ingress of Console sends, a send to
/// a member of a mob the host serves beside its launch mob is refused, and
/// so is its recovery: the host's ingress names an identity without its
/// mob, so it is asked nothing about another mob's member. The refusal is
/// the typed refusal of a mob id that says the host has the mob, and that
/// a governed send serves the launch mob only. Its kind is the one of a
/// mob the gateway does not serve to the caller, and the member `hosted`
/// tells the two apart: the same send for an id no mob has is answered
/// the mob id's refusal without it. Nothing is stored. A send to the
/// launch mob's member on the same console does reach the ingress.
async fn a_governed_send_to_a_hosted_mobs_member_is_refused(door: Door) {
    const KEY: &str = "door-send-governed";
    let ingress = Arc::new(CountingIngress::default());
    let installed: Arc<dyn crate::console_governed_ingress::ConsoleGovernedIngress> =
        ingress.clone();
    let host = TwoMobsOfOneName::start_with(Some(installed)).await;
    let second_mob = host.second.mob_id();
    let admin = DOOR_ROWS_ADMIN;

    let sent = ask_as(
        &host.app,
        door.send(&send_params(Some(second_mob.as_str()), SHARED_NAME, KEY)),
        admin,
    )
    .await;
    let recovered = ask_as(
        &host.app,
        door.recover(&recover_params(Some(second_mob.as_str()), SHARED_NAME, KEY)),
        admin,
    )
    .await;
    let asked_for_the_second_mob = ingress.asked();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let sent_to_no_mob_of_the_host = ask_as(
        &host.app,
        door.send(&send_params(Some(absent_mob.as_str()), SHARED_NAME, KEY)),
        admin,
    )
    .await;
    let of_the_second = host.sends_of(Some(second_mob.as_str())).await;
    let of_the_launch = host.sends_of(None).await;
    let _sent_with_no_mob = ask_as(
        &host.app,
        door.send(&send_params(None, SHARED_NAME, KEY)),
        admin,
    )
    .await;
    let asked_for_the_launch_mob = ingress.asked();
    host.shut_down().await;

    let mut wrong = Vec::new();
    for (what, answer) in [("send", &sent), ("recovery", &recovered)] {
        if !door.is_the_governed_refusal(answer, &second_mob) {
            wrong.push(format!(
                "the {what} is not answered the refusal of a governed send for a hosted mob: {} \
                 {}",
                answer.0, answer.2
            ));
        }
    }
    if !is_the_mob_ids_refusal(door.name(), &sent_to_no_mob_of_the_host, &absent_mob)
        || sent_to_no_mob_of_the_host.2.contains("hosted")
    {
        wrong.push(format!(
            "a send for an id no mob has is not answered the mob id's refusal without the \
             member hosted: {} {}",
            sent_to_no_mob_of_the_host.0, sent_to_no_mob_of_the_host.2
        ));
    }
    if asked_for_the_second_mob != 0 {
        wrong.push(format!(
            "the host's ingress was asked about the second mob's member: \
             {asked_for_the_second_mob} times"
        ));
    }
    if of_the_second != Ok(Vec::new()) || of_the_launch != Ok(Vec::new()) {
        wrong.push(format!(
            "a refused send left a frame: {of_the_second:?} / {of_the_launch:?}"
        ));
    }
    if asked_for_the_launch_mob == 0 {
        wrong.push(
            "the fixture: a send to the launch mob's member does not reach the host's ingress"
                .to_string(),
        );
    }
    assert!(
        wrong.is_empty(),
        "a governed send for a hosted mob, on {door:?}:\n{}",
        wrong.join("\n")
    );
}

#[tokio::test]
async fn a_governed_send_to_a_hosted_mobs_member_is_refused_on_the_request_door() {
    a_governed_send_to_a_hosted_mobs_member_is_refused(Door::RequestDoor).await;
}

#[tokio::test]
async fn a_governed_send_to_a_hosted_mobs_member_is_refused_on_the_routes() {
    a_governed_send_to_a_hosted_mobs_member_is_refused(Door::Route).await;
}

/// The uploads of a multipart send are written for the launch mob's
/// members alone: the multipart route takes a send's mob as every caller's
/// launch mob. An administrator's multipart send to a member of the second
/// mob is answered the mob id's refusal before anything is asked about that
/// mob and leaves no frame, and the same request with no mob is not
/// answered so.
#[tokio::test]
async fn a_multipart_send_to_a_hosted_mobs_member_is_refused() {
    const KEY: &str = "door-send-multipart";
    let host = TwoMobsOfOneName::start().await;
    let second_mob = host.second.mob_id();
    let for_the_second = ask_as(
        &host.app,
        multipart_request(
            "meta/console/send",
            &send_params(Some(second_mob.as_str()), SHARED_NAME, KEY),
        ),
        DOOR_ROWS_ADMIN,
    )
    .await;
    let asked_about_the_second_mob = asked_about(&host.resolver, &second_mob);
    let of_the_second = host.sends_of(Some(second_mob.as_str())).await;
    let of_the_launch = host.sends_of(None).await;
    let with_no_mob = ask_as(
        &host.app,
        multipart_request("meta/console/send", &send_params(None, SHARED_NAME, KEY)),
        DOOR_ROWS_ADMIN,
    )
    .await;
    host.shut_down().await;

    assert!(
        is_the_mob_ids_refusal("console multipart", &for_the_second, &second_mob),
        "a multipart send to a member of the second mob is refused: {for_the_second:?}"
    );
    assert_eq!(
        asked_about_the_second_mob, 0,
        "the resolver is asked nothing about the second mob for a multipart send"
    );
    assert_eq!(
        (of_the_second, of_the_launch),
        (Ok(Vec::new()), Ok(Vec::new())),
        "the refused multipart send left no frame"
    );
    assert!(
        !is_the_mob_ids_refusal("console multipart", &with_no_mob, &second_mob)
            && !with_no_mob.2.contains("foreign_mob_target"),
        "a multipart send that names no mob is not answered a mob id's refusal: {with_no_mob:?}"
    );
}

/// What the two doors answer an administrator's send, and then its
/// recovery, to the second mob's member of the shared name on `host`; and
/// the send frames the second mob's member and the launch mob's member of
/// that name hold afterwards. The host is shut down.
async fn sent_to_the_second_mobs_member(
    host: TwoMobsOfOneName,
) -> (Vec<(Door, &'static str, DoorAnswer)>, HeldSends, HeldSends) {
    let second_mob = host.second.mob_id();
    let mut answers = Vec::new();
    for door in Door::BOTH {
        let key = format!("door-send-to-the-second-mob-{door:?}");
        let sent = ask_as(
            &host.app,
            door.send(&send_params(Some(second_mob.as_str()), SHARED_NAME, &key)),
            DOOR_ROWS_ADMIN,
        )
        .await;
        answers.push((door, "send", sent));
        let recovered = ask_as(
            &host.app,
            door.recover(&recover_params(
                Some(second_mob.as_str()),
                SHARED_NAME,
                &key,
            )),
            DOOR_ROWS_ADMIN,
        )
        .await;
        answers.push((door, "recovery", recovered));
    }
    let of_the_second = host.sends_of(Some(second_mob.as_str())).await;
    let of_the_launch = host.sends_of(None).await;
    host.shut_down().await;
    (answers, of_the_second, of_the_launch)
}

/// A send and its recovery are held for a mob whose identity plane is
/// owed. The launch mob has an identity plane, so the launch composes one
/// for the mob it serves beside it; the second mob has none attached. An
/// administrator's send to the second mob's member, whose name a member of
/// the launch mob shares, and its recovery are answered the console's
/// refusal of a part that is not available, on the request door and on the
/// routes, and no member holds a send: the member is not reached past the
/// identity runtime that is to answer for it, and the launch mob's member
/// of the name gets nothing.
#[tokio::test]
async fn a_send_to_a_hosted_mob_whose_identity_plane_is_owed_is_held() {
    let (launch_dir, launch) = runtime_with_an_identity("launch").await;
    let (second_dir, second) = runtime_with_member("second", SHARED_NAME).await;
    let host = TwoMobsOfOneName::over(launch, second, vec![launch_dir, second_dir], None);
    let (answers, of_the_second, of_the_launch) = sent_to_the_second_mobs_member(host).await;

    let mut wrong = Vec::new();
    for (door, what, answer) in answers {
        let body = body_of(&answer);
        let held = match door {
            Door::RequestDoor => {
                body["error"]["code"] == json!(-32004) && body["error"]["data"].is_null()
            }
            Door::Route => {
                answer.0 == StatusCode::SERVICE_UNAVAILABLE && body["error"] == json!("unavailable")
            }
        };
        if !held {
            wrong.push(format!(
                "{door:?}: the {what} is not held: {} {}",
                answer.0, answer.2
            ));
        }
    }
    if of_the_second != Ok(Vec::new()) || of_the_launch != Ok(Vec::new()) {
        wrong.push(format!(
            "a held send left a frame: {of_the_second:?} / {of_the_launch:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a send to a hosted mob whose identity plane is owed:\n{}",
        wrong.join("\n")
    );
}

/// On a host whose launch composes no identity plane, a mob it serves
/// beside its launch mob has no member a Console send reaches: the member
/// lane serves the launch mob alone. An administrator's send to such a
/// mob's member, whose name a member of the launch mob shares, answers an
/// unknown identity on the request door and on the route, its recovery
/// finds no send, and no member holds a send: the launch mob's member of
/// the name gets nothing.
#[tokio::test]
async fn a_send_to_a_hosted_mob_without_an_identity_runtime_knows_no_such_identity() {
    let (launch_dir, launch) = runtime_with_member("launch", SHARED_NAME).await;
    let (second_dir, second) = runtime_with_member("second", SHARED_NAME).await;
    let host = TwoMobsOfOneName::over(launch, second, vec![launch_dir, second_dir], None);
    let (answers, of_the_second, of_the_launch) = sent_to_the_second_mobs_member(host).await;

    let mut wrong = Vec::new();
    for (door, what, answer) in answers {
        let body = body_of(&answer);
        let as_expected = match (what, door) {
            ("send", Door::RequestDoor) => body["error"]["code"] == json!(-32001),
            ("send", Door::Route) => {
                answer.0 == StatusCode::NOT_FOUND && body["error"] == json!("unknown_identity")
            }
            _ => door.found_no_send(&answer),
        };
        if !as_expected {
            wrong.push(format!(
                "{door:?}: the {what} is not answered an unknown identity, or its recovery a \
                 send that was not found: {} {}",
                answer.0, answer.2
            ));
        }
    }
    if of_the_second != Ok(Vec::new()) || of_the_launch != Ok(Vec::new()) {
        wrong.push(format!(
            "the send left a frame: {of_the_second:?} / {of_the_launch:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a send to a hosted mob without an identity runtime:\n{}",
        wrong.join("\n")
    );
}

/// A member of a mob the host serves beside its launch mob is named by its
/// identity there. The console's listing names an identity and its runtime
/// member id with no word of its mob, so an alias of it names no member of
/// such a mob: an administrator's send to the second mob that names its
/// member by that member's own current runtime member id answers an unknown
/// identity on the request door and on the route, and no member holds a
/// send.
#[tokio::test]
async fn a_send_to_a_hosted_mobs_member_by_an_alias_knows_no_such_identity() {
    let host = TwoMobsOfOneName::start().await;
    let second_mob = host.second.mob_id();
    let identity = crate::identity_first::AgentIdentity::parse(SHARED_NAME).expect("identity");
    let alias = match host.second.identity_runtime_ref() {
        Some(identity_runtime) => identity_runtime
            .status(&identity)
            .await
            .ok()
            .and_then(|status| status.agent_runtime_id)
            .map(|id| id.as_str().to_string()),
        None => None,
    };
    let mut answers = Vec::new();
    if let Some(alias) = alias.as_deref() {
        for door in Door::BOTH {
            let key = format!("door-send-by-alias-{door:?}");
            let answer = ask_as(
                &host.app,
                door.send(&send_params(Some(second_mob.as_str()), alias, &key)),
                DOOR_ROWS_ADMIN,
            )
            .await;
            answers.push((door, answer));
        }
    }
    let of_the_second = host.sends_of(Some(second_mob.as_str())).await;
    let of_the_launch = host.sends_of(None).await;
    host.shut_down().await;

    let mut wrong = Vec::new();
    if alias.as_deref().is_none_or(|alias| alias == SHARED_NAME) {
        wrong.push(format!(
            "the fixture: the second mob's member has no runtime member id of its own: {alias:?}"
        ));
    }
    for (door, answer) in answers {
        let body = body_of(&answer);
        let unknown_identity = match door {
            Door::RequestDoor => body["error"]["code"] == json!(-32001),
            Door::Route => {
                answer.0 == StatusCode::NOT_FOUND && body["error"] == json!("unknown_identity")
            }
        };
        if !unknown_identity {
            wrong.push(format!(
                "{door:?}: the send by an alias is not answered an unknown identity: {} {}",
                answer.0, answer.2
            ));
        }
    }
    if of_the_second != Ok(Vec::new()) || of_the_launch != Ok(Vec::new()) {
        wrong.push(format!(
            "the send left a frame: {of_the_second:?} / {of_the_launch:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a send to a hosted mob's member by an alias:\n{}",
        wrong.join("\n")
    );
}

/// A send and its recovery are member actions: for a mob whose identity
/// plane is owed they are held before they read their member, as the stop
/// of a member's run is, on the request door (both arms ask the hold) and
/// on the routes (both handlers take the route's mob through the one step
/// that asks it). Read from the source.
#[test]
fn a_send_and_its_recovery_are_held_for_a_mob_whose_identity_plane_is_owed() {
    let source = include_str!("http_console.rs");
    let function = |head: &str| {
        let text = source.find(head).map_or("", |from| &source[from..]);
        &text[..text.find("\n}\n").unwrap_or(text.len())]
    };
    let dispatch = function("async fn handle_console_runtime_rpc_with_visibility(");
    let arm = |head: &str, next: &str| {
        let arm = dispatch.split(head).nth(1).unwrap_or_default();
        arm.split(next).next().unwrap_or_default().to_string()
    };
    let hold = "member_action_held_for_an_owed_identity_plane(&governance.served_mob)";
    assert!(
        arm(
            "\"meta/console/send\" =>",
            "\"meta/console/recover_send\" =>"
        )
        .contains(hold),
        "the send's arm asks the hold"
    );
    assert!(
        arm("\"meta/console/recover_send\" =>", "\"meta/blob/get\" =>").contains(hold),
        "the recovery's arm asks the hold"
    );
    let step = function("async fn console_send_route_mob(");
    assert!(
        step.contains("member_action_held_for_an_owed_identity_plane("),
        "the step that states a send route's mob asks the hold"
    );
    for handler in [
        "async fn console_send_handler(",
        "async fn console_send_recover_handler(",
    ] {
        assert!(
            function(handler).contains("console_send_route_mob("),
            "{handler} takes its route's mob through the step that asks the hold"
        );
    }
}

/// Whether the status word `status` of a send's frame says that the send
/// was delivered: `delivered`, or `completed` once its turn ended.
fn was_delivered(status: &str) -> bool {
    matches!(status, "delivered" | "completed")
}

/// A send for a mob the host serves beside its launch mob is marked under
/// that mob. An administrator sends to the second mob's member and to the
/// launch mob's member of the same name; once each member's runtime
/// admitted its input, each send's frame says it was delivered: the second
/// mob's as the launch mob's does.
async fn a_hosted_mobs_send_is_marked_delivered_as_the_launch_mobs_is(door: Door) {
    let host = TwoMobsOfOneName::start().await;
    let second_mob = host.second.mob_id();
    let sent = ask_as(
        &host.app,
        door.send(&send_params(
            Some(second_mob.as_str()),
            SHARED_NAME,
            "door-send-marked-second",
        )),
        DOOR_ROWS_ADMIN,
    )
    .await;
    let sent_with_no_mob = ask_as(
        &host.app,
        door.send(&send_params(None, SHARED_NAME, "door-send-marked-launch")),
        DOOR_ROWS_ADMIN,
    )
    .await;
    let frame = door.accepted(&sent);
    let launch_frame = door.accepted(&sent_with_no_mob);
    let of_the_launch = host.settled_status(None, launch_frame.as_ref()).await;
    let of_the_second = host
        .settled_status(Some(second_mob.as_str()), frame.as_ref())
        .await;
    host.shut_down().await;

    assert!(
        was_delivered(&of_the_launch),
        "the fixture, on {door:?}: the launch mob's send is not marked delivered: {of_the_launch} \
         ({} {})",
        sent_with_no_mob.0,
        sent_with_no_mob.2
    );
    assert!(
        was_delivered(&of_the_second),
        "on {door:?}: the second mob's send is not marked delivered: {of_the_second} ({} {})",
        sent.0,
        sent.2
    );
}

#[tokio::test]
async fn a_hosted_mobs_send_is_marked_delivered_as_the_launch_mobs_is_on_the_request_door() {
    a_hosted_mobs_send_is_marked_delivered_as_the_launch_mobs_is(Door::RequestDoor).await;
}

#[tokio::test]
async fn a_hosted_mobs_send_is_marked_delivered_as_the_launch_mobs_is_on_the_routes() {
    a_hosted_mobs_send_is_marked_delivered_as_the_launch_mobs_is(Door::Route).await;
}

/// A rule by label of the second mob decides a send there at the first
/// request of a host: the door records the mob's live roster as that mob's
/// agents before the send rule is asked. The second mob's member carries a
/// label a rule keeps from the sends of the person whose rule gives the
/// send in that mob; the launch mob's member of the name does not carry
/// it. That person's send to the second mob's member, the first request
/// the host is asked, is refused by the access rules, as that refusal is
/// given in the launch mob, and leaves no frame; the same person's send to
/// the second mob's member that carries no label is accepted, and so is an
/// administrator's send to the member the rule keeps.
async fn a_label_rule_of_the_hosted_mob_decides_a_first_send(door: Door) {
    let (launch_dir, launch) = runtime_with_an_identity("launch").await;
    let (second_dir, second) = runtime_with_a_labelled_identity(
        "second",
        std::collections::BTreeMap::from([(
            KEPT_FROM_SENDS.0.to_string(),
            KEPT_FROM_SENDS.1.to_string(),
        )]),
    )
    .await;
    let host = TwoMobsOfOneName::over(launch, second, vec![launch_dir, second_dir], None);
    let second_mob = host.second.mob_id();
    let first = ask_as(
        &host.app,
        door.send(&send_params(
            Some(second_mob.as_str()),
            SHARED_NAME,
            "door-send-kept-by-a-label",
        )),
        SENDS_IN_THE_SECOND_MOB,
    )
    .await;
    let of_the_second = host.sends_of(Some(second_mob.as_str())).await;
    let in_the_launch = ask_as(
        &host.app,
        door.send(&send_params(None, SHARED_NAME, "door-send-kept-by-a-label")),
        SENDS_IN_THE_SECOND_MOB,
    )
    .await;
    let to_the_unlabelled = ask_as(
        &host.app,
        door.send(&send_params(
            Some(second_mob.as_str()),
            UNLABELLED_NAME,
            "door-send-no-label",
        )),
        SENDS_IN_THE_SECOND_MOB,
    )
    .await;
    let by_an_administrator = ask_as(
        &host.app,
        door.send(&send_params(
            Some(second_mob.as_str()),
            SHARED_NAME,
            "door-send-not-kept",
        )),
        DOOR_ROWS_ADMIN,
    )
    .await;
    host.shut_down().await;

    let mut wrong = Vec::new();
    if door.accepted(&first).is_some()
        || is_the_mob_ids_refusal(door.name(), &first, &second_mob)
        || door.refusal(&first) != door.refusal(&in_the_launch)
    {
        wrong.push(format!(
            "the first send to the member the label rule keeps is not refused by the access \
             rules ({}): {} {}",
            door.refusal(&in_the_launch),
            first.0,
            first.2
        ));
    }
    if of_the_second != Ok(Vec::new()) {
        wrong.push(format!("the refused send left a frame: {of_the_second:?}"));
    }
    if door.accepted(&to_the_unlabelled).is_none() {
        wrong.push(format!(
            "the same person's send to the second mob's member with no label is not \
             accepted: {} {}",
            to_the_unlabelled.0, to_the_unlabelled.2
        ));
    }
    if door.accepted(&by_an_administrator).is_none() {
        wrong.push(format!(
            "the fixture: an administrator's send to that member is not accepted: {} {}",
            by_an_administrator.0, by_an_administrator.2
        ));
    }
    assert!(
        wrong.is_empty(),
        "a label rule of a hosted mob at a first send, on {door:?}:\n{}",
        wrong.join("\n")
    );
}

#[tokio::test]
async fn a_label_rule_of_the_hosted_mob_decides_a_first_send_on_the_request_door() {
    a_label_rule_of_the_hosted_mob_decides_a_first_send(Door::RequestDoor).await;
}

#[tokio::test]
async fn a_label_rule_of_the_hosted_mob_decides_a_first_send_on_the_routes() {
    a_label_rule_of_the_hosted_mob_decides_a_first_send(Door::Route).await;
}

/// What asking a steer of a member until it takes one left: the accepted
/// steer's frame (none: no steer was accepted), how many steers the member's
/// runtime refused before it, and the last answer.
struct Steered {
    frame: Option<String>,
    refused: usize,
    last: DoorAnswer,
}

/// Steer the member [`SHARED_NAME`] of the mob `mob` (none: a steer that
/// names no mob) as an administrator, until the member takes a steer or
/// thirty seconds passed. A member whose start is not settled takes no
/// steer from a person: its runtime refuses the steer typed
/// (`host_human_input_unsupported`), after the door reserved the send, and
/// the door marks that send failed. Each steer is a send of its own key,
/// `key` and its number. Any other answer ends the asking.
async fn steer_until_taken(
    host: &TwoMobsOfOneName,
    door: Door,
    mob: Option<&str>,
    key: &str,
) -> Steered {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut refused = 0;
    loop {
        let mut params = send_params(mob, SHARED_NAME, &format!("{key}-{refused}"));
        params["handling_mode"] = json!("steer");
        let last = ask_as(&host.app, door.send(&params), DOOR_ROWS_ADMIN).await;
        let frame = door.accepted(&last);
        let body = body_of(&last);
        let not_started = match door {
            Door::RequestDoor => &body["error"]["data"]["kind"],
            Door::Route => &body["data"]["kind"],
        } == &json!("host_human_input_unsupported");
        if frame.is_some() || !not_started || tokio::time::Instant::now() >= deadline {
            return Steered {
                frame,
                refused,
                last,
            };
        }
        refused += 1;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// A steer to a member of a mob the host serves beside its launch mob is
/// that mob's send to its end. An administrator steers the second mob's
/// member, which shares its name with a member of the launch mob, until the
/// member takes a steer. The accepted steer's frame, under the second mob,
/// is marked delivered and has the terminal of a delivered steer; each
/// steer the member's runtime refused before it is marked failed, under the
/// second mob too; and the launch mob's member of the name holds no send
/// frame and no terminal. The same steer to the launch mob's member then
/// ends the same way under the launch mob, and leaves the second mob's
/// member the send frames it had.
async fn a_steer_to_a_hosted_mobs_member_is_delivered_under_that_mob(door: Door) {
    let host = TwoMobsOfOneName::start().await;
    let second_mob = host.second.mob_id();
    let to_the_second =
        steer_until_taken(&host, door, Some(second_mob.as_str()), "door-steer-second").await;
    let second_status = host
        .settled_status(Some(second_mob.as_str()), to_the_second.frame.as_ref())
        .await;
    let second_terminal = host
        .holds_a_steer_terminal(Some(second_mob.as_str()), to_the_second.frame.as_ref())
        .await;
    let second_frames = host.frames_of(Some(second_mob.as_str())).await;
    let second_sends = host.sends_of(Some(second_mob.as_str())).await;
    let launch_sends_before = host.sends_of(None).await;
    let launch_terminals_before = host.frames_of(None).await.map(|frames| {
        frames
            .iter()
            .filter(|held| held.kind == "interaction_complete")
            .count()
    });
    let to_the_launch = steer_until_taken(&host, door, None, "door-steer-launch").await;
    let launch_status = host
        .settled_status(None, to_the_launch.frame.as_ref())
        .await;
    let launch_terminal = host
        .holds_a_steer_terminal(None, to_the_launch.frame.as_ref())
        .await;
    let second_sends_after = host.sends_of(Some(second_mob.as_str())).await;
    host.shut_down().await;

    assert!(
        was_delivered(&launch_status) && launch_terminal,
        "the fixture, on {door:?}: the launch mob's steer is not delivered with its terminal: \
         {launch_status}, terminal {launch_terminal}, after {} refused ({} {})",
        to_the_launch.refused,
        to_the_launch.last.0,
        to_the_launch.last.2
    );
    // The second mob's send frames, as read right after its steer: each
    // with its status.
    let statuses: Vec<(String, String)> = second_frames
        .as_ref()
        .map(|frames| {
            frames
                .iter()
                .filter(|held| held.kind == "user_input")
                .map(|held| (held.id.clone(), held.status.as_str().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let marked_failed = statuses
        .iter()
        .filter(|(_, status)| status == "delivery_failed")
        .count();
    let mut wrong = Vec::new();
    if to_the_second.frame.is_none() {
        wrong.push(format!(
            "no steer is accepted after {} refused: {} {}",
            to_the_second.refused, to_the_second.last.0, to_the_second.last.2
        ));
    }
    if !was_delivered(&second_status) {
        wrong.push(format!(
            "the steer's frame is not marked delivered under the second mob: {second_status}"
        ));
    }
    if !second_terminal {
        wrong.push(
            "the second mob's member holds no terminal of a delivered steer for the frame"
                .to_string(),
        );
    }
    if marked_failed != to_the_second.refused || !all_name(&second_sends, Some(second_mob.as_str()))
    {
        wrong.push(format!(
            "the {} steers the member's runtime refused are not each one send frame of the \
             second mob marked failed: {statuses:?}, {second_sends:?}",
            to_the_second.refused
        ));
    }
    if launch_sends_before != Ok(Vec::new()) || launch_terminals_before != Ok(0) {
        wrong.push(format!(
            "the launch mob's member of the name is not untouched: sends \
             {launch_sends_before:?}, terminals {launch_terminals_before:?}"
        ));
    }
    if second_sends_after != second_sends {
        wrong.push(format!(
            "the launch mob's steer changed the second mob's send frames: {second_sends:?} \
             then {second_sends_after:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a steer to the second mob's member, on {door:?}:\n{}\nthe second mob's member holds: {:?}",
        wrong.join("\n"),
        second_frames.map(|frames| {
            frames
                .into_iter()
                .map(|held| format!("{} {} {}", held.kind, held.status.as_str(), held.payload))
                .collect::<Vec<_>>()
        })
    );
}

#[tokio::test]
async fn a_steer_to_a_hosted_mobs_member_is_delivered_under_that_mob_on_the_request_door() {
    a_steer_to_a_hosted_mobs_member_is_delivered_under_that_mob(Door::RequestDoor).await;
}

#[tokio::test]
async fn a_steer_to_a_hosted_mobs_member_is_delivered_under_that_mob_on_the_routes() {
    a_steer_to_a_hosted_mobs_member_is_delivered_under_that_mob(Door::Route).await;
}

/// A send whose delivery fails is marked failed under the mob it is for.
/// Both mobs are stopped, so that their identities stand and nothing is
/// delivered; an administrator sends to the second mob's member and to the
/// launch mob's member of the same name. The launch mob's send frame ends
/// `delivery_failed`, and the second mob's ends as it does.
#[tokio::test]
async fn a_hosted_mobs_failed_send_is_marked_failed_as_the_launch_mobs_is() {
    let host = TwoMobsOfOneName::start().await;
    let second_mob = host.second.mob_id();
    let _ = host.launch.mob_runtime().handle().stop().await;
    let _ = host.second.mob_runtime().handle().stop().await;
    let door = Door::RequestDoor;
    let to_the_second = ask_as(
        &host.app,
        door.send(&send_params(
            Some(second_mob.as_str()),
            SHARED_NAME,
            "door-send-fails-second",
        )),
        DOOR_ROWS_ADMIN,
    )
    .await;
    let to_the_launch = ask_as(
        &host.app,
        door.send(&send_params(None, SHARED_NAME, "door-send-fails-launch")),
        DOOR_ROWS_ADMIN,
    )
    .await;
    let (_, launch_status) = host.one_send_settled(None).await;
    let (_, second_status) = host.one_send_settled(Some(second_mob.as_str())).await;
    host.shut_down().await;

    assert_eq!(
        launch_status, "delivery_failed",
        "the fixture: the launch mob's send to a stopped mob is not marked failed ({} {})",
        to_the_launch.0, to_the_launch.2
    );
    assert_eq!(
        second_status, launch_status,
        "the second mob's failed send is not marked as the launch mob's is ({} {})",
        to_the_second.0, to_the_second.2
    );
}
