//! Whose a gating entry and a question set are: the member's that held the
//! origin's name when the entry was made.
//!
//! The launch is the gateway-shaped one of the grandparent module, with real
//! seatings and ends in its journal; the Console requires sign-in and has a
//! gating owner of its own and an access configuration under which every
//! signed-in person reads and decides gating and reads decisions. The
//! people: the owner of the private side chat, another person, and an
//! administrator. A row runs to its end and then fails with every
//! expectation it found unmet.

use std::collections::BTreeSet;

use super::*;
use crate::http_console::console_human_admission_tests::BOB;
use crate::http_console::console_private_side_chat_tests::{
    ALICE, ROOT, bob_claims, open_console, root_claims,
};

type GatingOwner = Arc<tokio::sync::Mutex<MetaRuntimeHandle>>;

/// Every signed-in person views and drives every agent, reads and decides
/// gating and reads decisions; `ROOT` administers; the owner and the other
/// person hold the approver id of question sets.
fn everyone_reads_and_decides() -> AccessController {
    let config: crate::access::AccessControlConfig = serde_json::from_value(json!({
        "enabled": true,
        "admins": [ROOT],
        "rules": [
            {
                "id": "everyone-drives-every-agent",
                "subjects": ["*"],
                "actions": ["agent.*"],
                "agents": ["*"],
            },
            {
                "id": "everyone-reads-and-decides",
                "subjects": ["*"],
                "actions": [
                    crate::access::ACTION_GATING_VIEW,
                    crate::access::ACTION_GATING_DECIDE,
                    crate::access::ACTION_DECISIONS_VIEW,
                    crate::access::ACTION_RUNTIME_ADMIN,
                ],
            },
        ],
        "approvers": { "ask_user": { "subjects": [ALICE, BOB] } },
    }))
    .expect("access configuration");
    AccessController::new(config).expect("access controller")
}

fn gating_owner(name: &str) -> GatingOwner {
    Arc::new(tokio::sync::Mutex::new(
        crate::start_meta_runtime(
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: format!("{name}-gating"),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            vec![],
            Duration::from_secs(1),
        )
        .expect("gating owner"),
    ))
}

/// A launch with the shared agent `alpha`, a Console with sign-in over it,
/// and three people.
struct Scene {
    launch: Launch,
    app: Router,
    gating: GatingOwner,
    questions: Questions,
    /// The owner of the private side chat.
    alice: String,
    /// Another person.
    bob: String,
    /// The administrator.
    root: String,
}

async fn scene(label: &str) -> Scene {
    scene_under(label, everyone_reads_and_decides()).await
}

/// [`scene`], with the Console under the access configuration `access`.
async fn scene_under(label: &str, access: AccessController) -> Scene {
    let launch = boot(label).await;
    launch.shared("planner", "alpha").await;
    let questions = serve_questions(launch.mob());
    let gating = gating_owner(&launch.mob_id);
    Scene {
        app: console_under(launch.mob(), &gating, questions.decisions.clone(), access),
        launch,
        gating,
        questions,
        alice: bearer(alice_claims()),
        bob: bearer(bob_claims()),
        root: bearer(root_claims()),
    }
}

/// The Console with sign-in over `runtime`, with the gating owner `gating`
/// and the durable decisions of `questions`.
fn console_over(runtime: &MobRuntime, gating: &GatingOwner, questions: &Questions) -> Router {
    console_with(runtime, gating, questions.decisions.clone())
}

/// The Console with sign-in over `runtime`, with the gating owner `gating`
/// and the durable decisions `decisions`.
fn console_with(runtime: &MobRuntime, gating: &GatingOwner, decisions: RuntimeDecisions) -> Router {
    console_under(runtime, gating, decisions, everyone_reads_and_decides())
}

/// [`console_with`], under the access configuration `access`.
fn console_under(
    runtime: &MobRuntime,
    gating: &GatingOwner,
    decisions: RuntimeDecisions,
    access: AccessController,
) -> Router {
    let mut state = enforced_console(runtime, Some(access));
    state.module_runtime = Some(Arc::clone(gating));
    state.durable_decisions = decisions;
    console_json_router_with_state(state)
}

/// The action ids of the audit rows `bearer` is read on `app`.
async fn audit_actions_on(app: &Router, bearer: &str) -> BTreeSet<String> {
    let answer = console_rpc_as(
        app,
        Some(bearer),
        "meta/gating/audit",
        json!({ "limit": 512 }),
    )
    .await;
    answer["result"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("an audit read: {answer}"))
        .iter()
        .filter_map(|entry| entry["action_id"].as_str().map(str::to_string))
        .collect()
}

/// The gating change stream of a Console as one caller reads it.
struct ChangeStream {
    body: axum::body::BodyDataStream,
    text: String,
}

impl ChangeStream {
    /// The stream of `app` for the holder of `bearer`, read past the frame
    /// it opens with.
    async fn open(app: &Router, bearer: &str) -> Self {
        Self::open_as(app, Some(bearer)).await
    }

    /// The stream of `app` for the holder of `bearer`, or for the caller
    /// with no token, read past the frame it opens with.
    async fn open_as(app: &Router, bearer: Option<&str>) -> Self {
        use tower::ServiceExt;

        let mut request = axum::http::Request::builder().uri("/console/gating/stream");
        if let Some(bearer) = bearer {
            request = request.header(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {bearer}"),
            );
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(axum::body::Body::empty())
                    .expect("gating stream request"),
            )
            .await
            .expect("gating stream response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let mut stream = Self {
            body: response.into_body().into_data_stream(),
            text: String::new(),
        };
        assert!(
            stream.hears("gating.resync_required", 30).await,
            "the stream opens with a resync: {}",
            stream.text
        );
        stream.text.clear();
        stream
    }

    /// Whether the stream sends the event `event` within `seconds`.
    async fn hears(&mut self, event: &str, seconds: u64) -> bool {
        use futures::StreamExt;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        while !self.text.contains(event) {
            match tokio::time::timeout_at(deadline, self.body.next()).await {
                Ok(Some(Ok(chunk))) => self.text.push_str(&String::from_utf8_lossy(&chunk)),
                _ => return false,
            }
        }
        true
    }
}

/// The record of who a runtime's members are private to, unreadable: every
/// read of it fails.
struct UnreadableRecord;

#[async_trait::async_trait]
impl crate::private_owners::MemberSpawnLog for UnreadableRecord {
    async fn spawns_after(
        &self,
        _cursor: u64,
        _limit: usize,
    ) -> Result<Option<crate::private_owners::MemberSpawnPage>, String> {
        Err("the mob's event log cannot be read".to_string())
    }
}

/// The pending ids `bearer` is listed on `app`, or the refusal it is
/// answered.
async fn pending_on(app: &Router, bearer: &str) -> Result<BTreeSet<String>, Value> {
    let answer = console_rpc_as(app, Some(bearer), "meta/gating/pending", json!({})).await;
    match answer["result"]["pending"].as_array() {
        Some(entries) => Ok(entries
            .iter()
            .filter_map(|entry| entry["pending_id"].as_str().map(str::to_string))
            .collect()),
        None => Err(answer["error"].clone()),
    }
}

/// Whether `error` is the refusal that tells the caller to ask again: who
/// held a member's name when a record was made is still being read.
fn says_to_ask_again(error: &Value) -> bool {
    error["code"] == json!(crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE)
        && error["data"]["kind"] == json!("identity_history_unavailable")
        && error["data"]["retryable"] == json!(true)
}

/// How long the rows wait for the launch's event pump to show the record a
/// seating or an end.
const PUMP: Duration = Duration::from_secs(2);

impl Scene {
    /// The owner forks her private side chat of `alpha` under [`SIDE_NAME`].
    async fn fork_the_side_chat(&self) {
        let forked = fork_on_console(&self.app, Some(&self.alice), "alpha", SIDE_NAME).await;
        assert!(forked["error"].is_null(), "{forked:#}");
        tokio::time::sleep(PUMP).await;
    }

    /// The owner closes her side chat.
    async fn close_the_side_chat(&self) {
        close(&self.launch, &self.app, &self.alice, SIDE_NAME).await;
        tokio::time::sleep(PUMP).await;
    }

    /// The host seats a shared member under `name`.
    async fn seat_shared(&self, name: &str) {
        self.seat_shared_as("worker", name).await;
    }

    /// The host seats a shared member of the role `profile` under `name`.
    async fn seat_shared_as(&self, profile: &str, name: &str) {
        self.launch.shared(profile, name).await;
        tokio::time::sleep(PUMP).await;
    }

    async fn evaluate(&self, origin: Option<&str>, risk_tier: &str) -> (String, Option<String>) {
        let request = json!({
            "action": "Publish the reviewed report",
            "actor_id": "agent:writer",
            "risk_tier": risk_tier,
            "approval_timeout_ms": 600_000,
        });
        let result = self.gating.lock().await.evaluate_gating_action_with_origin(
            serde_json::from_value(request).expect("gating request"),
            origin.map(|identity| crate::runtime::GatingOrigin {
                identity: identity.to_string(),
                conversation_id: None,
                interaction_id: None,
            }),
        );
        (result.action_id, result.pending_id)
    }

    /// A pending request whose action came from the member `origin`: its
    /// pending id.
    async fn pending_from(&self, origin: &str) -> String {
        self.evaluate(Some(origin), "r3")
            .await
            .1
            .expect("an R3 gate waits for a decision")
    }

    /// An action of the member `origin` that was allowed at once: the id of
    /// the action its audit row names.
    async fn evaluated_from(&self, origin: &str) -> String {
        self.evaluate(Some(origin), "r1").await.0
    }

    /// The pending ids `bearer` is listed, or the refusal it is answered.
    async fn pending(&self, bearer: &str) -> Result<BTreeSet<String>, Value> {
        pending_on(&self.app, bearer).await
    }

    /// The action ids of the audit rows `bearer` is read.
    async fn audit_actions(&self, bearer: &str) -> BTreeSet<String> {
        audit_actions_on(&self.app, bearer).await
    }

    /// The question sets `bearer` is listed.
    async fn question_sets(&self, bearer: &str) -> BTreeSet<String> {
        let answer =
            console_rpc_as(&self.app, Some(bearer), "meta/gating/questions", json!({})).await;
        answer["result"]["question_sets"]
            .as_array()
            .unwrap_or_else(|| panic!("a list of question sets: {answer}"))
            .iter()
            .filter_map(|set| set["pending_id"].as_str().map(str::to_string))
            .collect()
    }

    /// `bearer` approves the pending request `pending_id`.
    async fn approve(&self, bearer: &str, pending_id: &str) -> Value {
        console_rpc_as(
            &self.app,
            Some(bearer),
            "meta/gating/decide",
            json!({ "pending_id": pending_id, "decision": "approve" }),
        )
        .await
    }

    async fn still_pending(&self, pending_id: &str) -> bool {
        self.gating
            .lock()
            .await
            .list_gating_pending()
            .iter()
            .any(|entry| entry.pending_id == pending_id)
    }

    /// A read of the decisions family for the decision `decision_id`.
    async fn decision(&self, bearer: &str, method: &str, decision_id: &str) -> Value {
        console_rpc_as(
            &self.app,
            Some(bearer),
            method,
            json!({ "decision_id": decision_id }),
        )
        .await
    }

    fn expect(&self, met: bool, what: impl Into<String>) {
        self.launch.expect(met, what);
    }
}

fn set_of(ids: &[&String]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).clone()).collect()
}

/// The code and the data of a refusal, without its message (which names the
/// id asked for).
fn refusal(answer: &Value) -> Value {
    json!({ "code": answer["error"]["code"], "data": answer["error"]["data"] })
}

/// What a member private to a person recorded under its name stays that
/// person's when another member holds the name later. The owner's private
/// side chat is the origin of a pending request and of an audit row; after
/// it is closed, a request and a question set are recorded under its name
/// while nobody holds it; then a shared member is seated under the name and
/// is the origin of a request, an audit row and a question set of its own.
/// Another person is listed, reads and decides the later member's entries
/// and none of the side chat's; the owner and the administrator are listed
/// all of them, and the owner decides hers.
#[tokio::test(flavor = "multi_thread")]
async fn what_a_private_member_recorded_under_its_name_stays_its_owners_under_a_later_holder() {
    let scene = scene("origin-later-holder").await;
    let mob = scene.launch.mob();
    scene.fork_the_side_chat().await;
    let of_the_side_chat = scene.pending_from(SIDE_NAME).await;
    let audit_of_the_side_chat = scene.evaluated_from(SIDE_NAME).await;
    scene.close_the_side_chat().await;
    let under_the_unheld_name = scene.pending_from(SIDE_NAME).await;
    let set_under_the_unheld_name = scene.questions.asked(mob, SIDE_NAME, "call-unheld").await;
    scene.seat_shared(SIDE_NAME).await;
    let of_the_later_member = scene.pending_from(SIDE_NAME).await;
    let audit_of_the_later_member = scene.evaluated_from(SIDE_NAME).await;
    let set_of_the_later_member = scene.questions.asked(mob, SIDE_NAME, "call-later").await;
    tokio::time::sleep(PUMP).await;

    // The other person: the later member's entries, none of the side chat's.
    let others_pending = scene.pending(&scene.bob).await;
    scene.expect(
        others_pending == Ok(set_of(&[&of_the_later_member])),
        format!("the other person is listed the later member's request only: {others_pending:?}"),
    );
    let others_audit = scene.audit_actions(&scene.bob).await;
    scene.expect(
        others_audit.contains(&audit_of_the_later_member)
            && !others_audit.contains(&audit_of_the_side_chat),
        format!(
            "the other person reads the later member's audit row and not the side chat's: \
             {others_audit:?}"
        ),
    );
    let others_sets = scene.question_sets(&scene.bob).await;
    scene.expect(
        others_sets == set_of(&[&set_of_the_later_member]),
        format!("the other person is listed the later member's set only: {others_sets:?}"),
    );
    let unknown = scene.approve(&scene.bob, "gate-pending-of-nobody").await;
    for (what, pending_id) in [
        ("the side chat's request", &of_the_side_chat),
        ("the request under the unheld name", &under_the_unheld_name),
    ] {
        let decided = scene.approve(&scene.bob, pending_id).await;
        scene.expect(
            refusal(&decided) == refusal(&unknown) && scene.still_pending(pending_id).await,
            format!(
                "the other person's decide of {what} is answered as an id that is not pending, \
                 and the request stays pending: {decided}"
            ),
        );
    }
    let others_answer = answer(
        &scene.app,
        &scene.bob,
        &scene.questions,
        &set_under_the_unheld_name,
    )
    .await;
    scene.expect(
        !others_answer["error"].is_null()
            && scene.questions.phase(&set_under_the_unheld_name)
                == meerkat_core::approval::decision::DecisionPhase::Open,
        format!("the other person does not answer the set under the unheld name: {others_answer}"),
    );

    // The owner and the administrator: everything.
    let everything = set_of(&[
        &of_the_side_chat,
        &under_the_unheld_name,
        &of_the_later_member,
    ]);
    for (who, bearer) in [
        ("the owner", &scene.alice),
        ("the administrator", &scene.root),
    ] {
        let pending = scene.pending(bearer).await;
        scene.expect(
            pending == Ok(everything.clone()),
            format!("{who} is listed every request: {pending:?}"),
        );
        let audit = scene.audit_actions(bearer).await;
        scene.expect(
            audit.contains(&audit_of_the_side_chat) && audit.contains(&audit_of_the_later_member),
            format!("{who} reads both audit rows: {audit:?}"),
        );
    }
    let owners_sets = scene.question_sets(&scene.alice).await;
    scene.expect(
        owners_sets == set_of(&[&set_under_the_unheld_name, &set_of_the_later_member]),
        format!("the owner is listed both sets: {owners_sets:?}"),
    );

    // The later member's own entries are decided as before; the owner
    // decides hers.
    let others_own = scene.approve(&scene.bob, &of_the_later_member).await;
    scene.expect(
        others_own["error"].is_null() && !scene.still_pending(&of_the_later_member).await,
        format!("the other person decides the later member's request: {others_own}"),
    );
    let others_own_set = answer(
        &scene.app,
        &scene.bob,
        &scene.questions,
        &set_of_the_later_member,
    )
    .await;
    scene.expect(
        others_own_set["result"]["outcome"] == json!("answered"),
        format!("the other person answers the later member's set: {others_own_set}"),
    );
    let owners = scene.approve(&scene.alice, &of_the_side_chat).await;
    scene.expect(
        owners["error"].is_null() && !scene.still_pending(&of_the_side_chat).await,
        format!("the owner decides her side chat's request: {owners}"),
    );
    let owners_set = answer(
        &scene.app,
        &scene.alice,
        &scene.questions,
        &set_under_the_unheld_name,
    )
    .await;
    scene.expect(
        owners_set["result"]["outcome"] == json!("answered"),
        format!("the owner answers the set under the unheld name: {owners_set}"),
    );
    scene.launch.finish().await;
}

/// An ended question set is read through the decisions family by its
/// asker's owner and the administrator only. The owner's private side chat
/// asks; she closes it, which withdraws the set; a shared member is seated
/// under the name. Another person who holds the approver id of question
/// sets is answered, by the get, the rendering and the list, as for a
/// decision that is not there; a decide by the set's id is answered as an
/// id that is not pending.
#[tokio::test(flavor = "multi_thread")]
async fn an_ended_question_set_is_read_through_the_decisions_family_by_its_askers_owner_only() {
    let scene = scene("origin-ended-set").await;
    let mob = scene.launch.mob();
    scene.fork_the_side_chat().await;
    let asked = scene.questions.asked(mob, SIDE_NAME, "call-1").await;
    scene.close_the_side_chat().await;
    scene.expect(
        scene.questions.withdrawn_within(&asked, 10).await,
        "the closed side chat's set is withdrawn",
    );
    scene.seat_shared(SIDE_NAME).await;

    let unknown = format!("decision:{}", "0".repeat(64));
    for method in ["meta/decisions/get", "meta/decisions/rendering"] {
        let others = scene.decision(&scene.bob, method, &asked).await;
        let others_unknown = scene.decision(&scene.bob, method, &unknown).await;
        scene.expect(
            !others_unknown["error"].is_null() && refusal(&others) == refusal(&others_unknown),
            format!("{method}: the other person is answered as for no decision: {others}"),
        );
        for (who, bearer) in [
            ("the owner", &scene.alice),
            ("the administrator", &scene.root),
        ] {
            let read = scene.decision(bearer, method, &asked).await;
            scene.expect(
                read["error"].is_null(),
                format!("{method}: {who} reads the ended set: {read}"),
            );
        }
    }
    let others_list = console_rpc_as(
        &scene.app,
        Some(&scene.bob),
        "meta/decisions/list",
        json!({}),
    )
    .await;
    scene.expect(
        others_list["error"].is_null() && !others_list.to_string().contains(&asked),
        format!("the other person's list names the set nowhere: {others_list}"),
    );
    let others_decide = answer(&scene.app, &scene.bob, &scene.questions, &asked).await;
    // The same request for an id that names nothing (it presents the set's
    // own digest: no decision row is read for an id that has none).
    let unknown_decide = console_rpc_as(
        &scene.app,
        Some(&scene.bob),
        "meta/gating/decide",
        json!({
            "pending_id": unknown,
            "decision": "approve",
            "presented_digest": scene.questions.row(&asked).descriptor.subject_digest.to_string(),
            "answers": [{"question_id": "q1", "answer": {"kind": "choice", "choice_id": "a"}}],
        }),
    )
    .await;
    scene.expect(
        refusal(&others_decide) == refusal(&unknown_decide),
        format!(
            "the other person's decide by the set's id is answered as an id that is not \
             pending: {others_decide}"
        ),
    );
    scene.launch.finish().await;
}

/// A name private to one person, then shared, then private to another: no
/// entry reaches the other owner. The first owner's side chat is the origin
/// of a request; a shared member under the name is the origin of another;
/// then a member private to the other person holds the name and is the
/// origin of a third. The other person is listed the shared member's and
/// their own; the first owner none (her side chat's request is shown to the
/// administrator only while another person's member holds the name); the
/// administrator all three.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_private_to_one_person_and_then_to_another_shows_neither_the_others_entries() {
    let scene = scene("origin-two-owners").await;
    let mob = scene.launch.mob();
    // The other person's private-owner key, read from a side chat of theirs.
    let others_side = fork_on_console(&scene.app, Some(&scene.bob), "alpha", "beta-side").await;
    assert!(others_side["error"].is_null(), "{others_side:#}");
    let others_key = scene
        .launch
        .entry("beta-side")
        .await
        .and_then(|seat| seat.private_to)
        .expect("the other person's side chat is private to them");

    scene.fork_the_side_chat().await;
    let of_the_first_owner = scene.pending_from(SIDE_NAME).await;
    scene.close_the_side_chat().await;
    scene.seat_shared(SIDE_NAME).await;
    let of_the_shared_member = scene.pending_from(SIDE_NAME).await;
    mob.handle()
        .retire(AgentIdentity::from(SIDE_NAME))
        .await
        .expect("retire the shared member");
    scene.expect(
        scene.launch.gone_within(SIDE_NAME, 10).await,
        "the roster no longer holds the shared member",
    );
    tokio::time::sleep(PUMP).await;
    mob.handle()
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("planner", AgentIdentity::from(SIDE_NAME))
                .with_labels(std::collections::BTreeMap::from([(
                    PRIVATE_TO.to_string(),
                    others_key,
                )])),
        )
        .await
        .expect("seat a member private to the other person");
    tokio::time::sleep(PUMP).await;
    let of_the_second_owner = scene.pending_from(SIDE_NAME).await;
    tokio::time::sleep(PUMP).await;

    let others = scene.pending(&scene.bob).await;
    scene.expect(
        others == Ok(set_of(&[&of_the_shared_member, &of_the_second_owner])),
        format!(
            "the second owner is listed the shared member's request and their own, not the \
             first owner's: {others:?}"
        ),
    );
    let first_owners = scene.pending(&scene.alice).await;
    scene.expect(
        first_owners == Ok(BTreeSet::new()),
        format!(
            "the first owner is listed none while the name is another person's: {first_owners:?}"
        ),
    );
    let administrators = scene.pending(&scene.root).await;
    scene.expect(
        administrators
            == Ok(set_of(&[
                &of_the_first_owner,
                &of_the_shared_member,
                &of_the_second_owner,
            ])),
        format!("the administrator is listed all three: {administrators:?}"),
    );
    scene.launch.finish().await;
}

/// A request recorded before any member held its origin's name. Under a
/// name only a shared member holds later, it is shown and decided as
/// before. Under a name a member private to a person holds later, whose it
/// was cannot be told: it is shown to the administrator only, and neither
/// the owner nor another person decides it.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_made_before_any_member_held_its_origins_name() {
    let scene = scene("origin-before-first").await;
    let under_a_shared_name = scene.pending_from("gamma").await;
    let under_a_private_name = scene.pending_from(SIDE_NAME).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    scene.seat_shared("gamma").await;
    scene.fork_the_side_chat().await;

    for (who, bearer) in [("the owner", &scene.alice), ("another person", &scene.bob)] {
        let pending = scene.pending(bearer).await;
        scene.expect(
            pending
                .as_ref()
                .is_ok_and(|pending| pending.contains(&under_a_shared_name)),
            format!("{who} is listed the request under the shared name: {pending:?}"),
        );
        scene.expect(
            pending
                .as_ref()
                .is_ok_and(|pending| !pending.contains(&under_a_private_name)),
            format!("{who} is not listed the request under the private name: {pending:?}"),
        );
        let decided = scene.approve(bearer, &under_a_private_name).await;
        scene.expect(
            !decided["error"].is_null() && scene.still_pending(&under_a_private_name).await,
            format!("{who} does not decide the request under the private name: {decided}"),
        );
    }
    let administrators = scene.pending(&scene.root).await;
    scene.expect(
        administrators == Ok(set_of(&[&under_a_shared_name, &under_a_private_name])),
        format!("the administrator is listed both: {administrators:?}"),
    );
    let decided = scene.approve(&scene.bob, &under_a_shared_name).await;
    scene.expect(
        decided["error"].is_null() && !scene.still_pending(&under_a_shared_name).await,
        format!("another person decides the request under the shared name: {decided}"),
    );
    scene.launch.finish().await;
}

/// A pending request whose opening audit row is no longer among the rows
/// the audit retains: when it was made is not known. Under the name of a
/// shared agent no private member ever held, it stays listed and is decided
/// as before. Under a name a member private to a person holds, it is shown
/// to the administrator only.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_whose_opening_audit_row_is_no_longer_retained() {
    let scene = scene("origin-opening-row-gone").await;
    scene.fork_the_side_chat().await;
    let of_a_shared_agent = scene.pending_from("alpha").await;
    let of_the_side_chat = scene.pending_from(SIDE_NAME).await;
    // More rows than the audit retains push both opening rows out.
    for _ in 0..520 {
        scene.evaluate(None, "r1").await;
    }
    let retained = scene.gating.lock().await.gating_audit_entries(usize::MAX);
    scene.expect(
        !retained
            .iter()
            .any(|entry| entry.pending_id.as_deref() == Some(of_the_side_chat.as_str())),
        "the audit no longer retains the requests' opening rows",
    );

    for (who, bearer) in [("the owner", &scene.alice), ("another person", &scene.bob)] {
        let pending = scene.pending(bearer).await;
        scene.expect(
            pending == Ok(set_of(&[&of_a_shared_agent])),
            format!("{who} is listed the shared agent's request only: {pending:?}"),
        );
    }
    let administrators = scene.pending(&scene.root).await;
    scene.expect(
        administrators == Ok(set_of(&[&of_a_shared_agent, &of_the_side_chat])),
        format!("the administrator is listed both: {administrators:?}"),
    );
    let decided = scene.approve(&scene.bob, &of_a_shared_agent).await;
    scene.expect(
        decided["error"].is_null() && !scene.still_pending(&of_a_shared_agent).await,
        format!("another person decides the shared agent's request: {decided}"),
    );
    scene.launch.finish().await;
}

/// A Console that identifies nobody: its one operator reads the requests
/// and the question sets of the runtime's members as before, under a name
/// that was seated more than once too.
#[tokio::test(flavor = "multi_thread")]
async fn on_a_console_that_identifies_nobody_the_operator_reads_its_members_entries() {
    let launch = boot("origin-open-console").await;
    launch.shared("planner", "alpha").await;
    let questions = serve_questions(launch.mob());
    let gating = gating_owner(&launch.mob_id);
    let mut state = open_console(launch.mob());
    state.module_runtime = Some(Arc::clone(&gating));
    let app = console_json_router_with_state(state);
    let evaluate = |origin: &'static str| {
        let gating = Arc::clone(&gating);
        async move {
            let request = json!({
                "action": "Publish the reviewed report",
                "actor_id": "agent:writer",
                "risk_tier": "r3",
                "approval_timeout_ms": 600_000,
            });
            gating
                .lock()
                .await
                .evaluate_gating_action_with_origin(
                    serde_json::from_value(request).expect("gating request"),
                    Some(crate::runtime::GatingOrigin {
                        identity: origin.to_string(),
                        conversation_id: None,
                        interaction_id: None,
                    }),
                )
                .pending_id
                .expect("an R3 gate waits for a decision")
        }
    };
    let first = evaluate("alpha").await;
    launch
        .mob()
        .handle()
        .respawn(AgentIdentity::from("alpha"), None)
        .await
        .expect("respawn the shared agent");
    tokio::time::sleep(PUMP).await;
    let second = evaluate("alpha").await;
    let asked = questions.asked(launch.mob(), "alpha", "call-1").await;
    tokio::time::sleep(PUMP).await;

    let pending = console_rpc_as(&app, None, "meta/gating/pending", json!({})).await;
    let listed: BTreeSet<String> = pending["result"]["pending"]
        .as_array()
        .unwrap_or_else(|| panic!("a pending list: {pending}"))
        .iter()
        .filter_map(|entry| entry["pending_id"].as_str().map(str::to_string))
        .collect();
    launch.expect(
        listed == set_of(&[&first, &second]),
        format!("the operator is listed both requests: {listed:?}"),
    );
    let sets = console_rpc_as(&app, None, "meta/gating/questions", json!({})).await;
    launch.expect(
        sets["result"]["question_sets"][0]["pending_id"] == json!(asked),
        format!("the operator is listed the set: {sets}"),
    );
    launch.finish().await;
}

/// The requests a person makes on the gating doors, each with its
/// parameters: the reads, both decides (of a request and of a question
/// set), and the decisions family.
fn door_requests(pending_id: &str, decision_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("meta/gating/pending", json!({})),
        ("meta/gating/audit", json!({ "limit": 512 })),
        ("meta/gating/questions", json!({})),
        ("meta/delivery/history", json!({})),
        (
            "meta/gating/decide",
            json!({ "pending_id": pending_id, "decision": "approve" }),
        ),
        (
            "meta/gating/decide",
            json!({ "pending_id": decision_id, "decision": "reject" }),
        ),
        ("meta/decisions/list", json!({})),
        ("meta/decisions/get", json!({ "decision_id": decision_id })),
        (
            "meta/decisions/rendering",
            json!({ "decision_id": decision_id }),
        ),
    ]
}

/// While the first read of who a runtime's members are private to is owed,
/// every request of a person on the gating doors is answered the refusal
/// that says to ask again, whatever the owners hold: a Console whose gating
/// owner holds a request, an audit row and a question set, and one whose
/// owners hold nothing, answer the same. Nothing is decided meanwhile. The
/// administrator reads everything. Once the read has landed, no request is
/// answered the refusal.
#[tokio::test(flavor = "multi_thread")]
async fn while_who_held_a_name_is_still_being_read_a_person_is_told_to_ask_again() {
    let scene = scene("origin-first-read").await;
    let of_a_shared_agent = scene.pending_from("alpha").await;
    let audit_of_a_shared_agent = scene.evaluated_from("alpha").await;
    let asked = scene
        .questions
        .asked(scene.launch.mob(), "alpha", "call-1")
        .await;
    // A runtime over the same mob whose first read was tried and failed.
    let unread = MobRuntime::from_handle(scene.launch.mob().handle());
    assert!(
        !unread
            .private_owners()
            .seeded(&UnreadableRecord)
            .await
            .landed(),
        "the first read has not succeeded"
    );
    let app = console_with(&unread, &scene.gating, scene.questions.decisions.clone());
    // The same runtime's Console with owners that hold nothing.
    let nothing_held = console_with(
        &unread,
        &gating_owner("origin-first-read-nothing-held"),
        RuntimeDecisions::default(),
    );

    for (method, params) in door_requests(&of_a_shared_agent, &asked) {
        let held = console_rpc_as(&app, Some(&scene.bob), method, params.clone()).await;
        let not_held = console_rpc_as(&nothing_held, Some(&scene.bob), method, params).await;
        scene.expect(
            says_to_ask_again(&held["error"]),
            format!("{method} tells a person to ask again: {held}"),
        );
        scene.expect(
            held["error"] == not_held["error"],
            format!(
                "{method} answers the same where the owners hold nothing: {held} and {not_held}"
            ),
        );
    }
    scene.expect(
        scene.still_pending(&of_a_shared_agent).await
            && scene.questions.phase(&asked)
                == meerkat_core::approval::decision::DecisionPhase::Open,
        "nothing was decided meanwhile",
    );
    let administrators = pending_on(&app, &scene.root).await;
    scene.expect(
        administrators == Ok(set_of(&[&of_a_shared_agent])),
        format!("the administrator is listed the request meanwhile: {administrators:?}"),
    );

    // The read lands: no request is answered the refusal.
    assert!(
        unread
            .private_owners()
            .seeded(&scene.launch.mob().handle())
            .await
            .landed(),
        "the first read has succeeded"
    );
    for (method, params) in door_requests("gate-pending-of-nobody", &asked) {
        if method == "meta/gating/decide" && params["decision"] == json!("reject") {
            // Asked last, below: it dismisses the set.
            continue;
        }
        for (where_, console) in [
            ("the owners that hold", &app),
            ("no owner holding", &nothing_held),
        ] {
            let answer = console_rpc_as(console, Some(&scene.bob), method, params.clone()).await;
            scene.expect(
                !says_to_ask_again(&answer["error"]),
                format!("once the read has landed, {method} with {where_} answers: {answer}"),
            );
        }
    }
    let pending = pending_on(&app, &scene.bob).await;
    scene.expect(
        pending == Ok(set_of(&[&of_a_shared_agent])),
        format!("the person is listed the request: {pending:?}"),
    );
    let audit = audit_actions_on(&app, &scene.bob).await;
    scene.expect(
        audit.contains(&audit_of_a_shared_agent),
        format!("and reads the audit row: {audit:?}"),
    );
    let set = console_rpc_as(
        &app,
        Some(&scene.bob),
        "meta/decisions/get",
        json!({ "decision_id": asked }),
    )
    .await;
    scene.expect(
        set["error"].is_null(),
        format!("and reads the shared agent's question set: {set}"),
    );
    scene.launch.finish().await;
}

/// A runtime that reads the mob's journal at its start answers as the one
/// that saw the seatings and the ends as they happened. The owner's private
/// side chat is the origin of a request; she closes it; a shared member is
/// seated under the name and is the origin of another. A second runtime
/// over the same mob reads the journal once, and a Console over it lists
/// each person what the first Console lists them.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_reads_the_journal_at_its_start_answers_as_the_one_that_saw_it_happen() {
    let scene = scene("origin-restart").await;
    scene.fork_the_side_chat().await;
    let of_the_side_chat = scene.pending_from(SIDE_NAME).await;
    scene.close_the_side_chat().await;
    scene.seat_shared(SIDE_NAME).await;
    let of_the_later_member = scene.pending_from(SIDE_NAME).await;
    tokio::time::sleep(PUMP).await;

    let restarted = MobRuntime::from_handle(scene.launch.mob().handle());
    assert!(
        restarted
            .private_owners()
            .seeded(&scene.launch.mob().handle())
            .await
            .landed(),
        "the journal is read"
    );
    let app = console_over(&restarted, &scene.gating, &scene.questions);
    for (who, bearer, listed) in [
        (
            "the other person",
            &scene.bob,
            set_of(&[&of_the_later_member]),
        ),
        (
            "the owner",
            &scene.alice,
            set_of(&[&of_the_side_chat, &of_the_later_member]),
        ),
        (
            "the administrator",
            &scene.root,
            set_of(&[&of_the_side_chat, &of_the_later_member]),
        ),
    ] {
        let live = scene.pending(bearer).await;
        let read = pending_on(&app, bearer).await;
        scene.expect(
            live == Ok(listed.clone()) && read == Ok(listed),
            format!("{who}: as it happened {live:?}, read at the start {read:?}"),
        );
    }
    scene.launch.finish().await;
}

/// While the first read of who a runtime's members are private to is owed,
/// the gating change stream sends a person no frame, whatever rows are
/// recorded; the administrator's stream tells of the row. Once the read has
/// landed, the stream's own periodic re-check decides the rows it held and
/// tells the person of the one they may see.
#[tokio::test(flavor = "multi_thread")]
async fn the_gating_change_stream_sends_a_person_no_frame_while_who_held_a_name_is_still_being_read()
 {
    let scene = scene("origin-first-read-stream").await;
    let unread = MobRuntime::from_handle(scene.launch.mob().handle());
    assert!(
        !unread
            .private_owners()
            .seeded(&UnreadableRecord)
            .await
            .landed(),
        "the first read has not succeeded"
    );
    let app = console_with(&unread, &scene.gating, scene.questions.decisions.clone());
    let mut persons = ChangeStream::open(&app, &scene.bob).await;
    let mut administrators = ChangeStream::open(&app, &scene.root).await;

    scene.evaluated_from("alpha").await;
    scene.expect(
        administrators.hears("gating.changed", 20).await,
        format!(
            "the administrator's stream tells of the row: {}",
            administrators.text
        ),
    );
    scene.expect(
        !persons.hears("gating.changed", 3).await,
        format!(
            "the person's stream sends no frame meanwhile: {}",
            persons.text
        ),
    );

    assert!(
        unread
            .private_owners()
            .seeded(&scene.launch.mob().handle())
            .await
            .landed(),
        "the first read has succeeded"
    );
    scene.expect(
        persons.hears("gating.changed", 40).await,
        format!(
            "once the read has landed, the stream's re-check tells the person of the row it \
             held: {}",
            persons.text
        ),
    );
    scene.launch.finish().await;
}

/// On a Console that identifies nobody, the one operator is answered while
/// the first read of who members are private to is owed as when it is not:
/// no gating door answers the refusal that says to ask again, the gating
/// change stream tells of a shared agent's row, the operator is listed a
/// shared agent's request, and the pending list and both decides answer
/// the same once the read has landed (such a Console takes no decide from a
/// caller it cannot name, read owed or not).
#[tokio::test(flavor = "multi_thread")]
async fn on_a_console_that_identifies_nobody_the_operator_is_answered_while_who_held_a_name_is_still_being_read()
 {
    let scene = scene("origin-first-read-open-console").await;
    let listed = scene.pending_from("alpha").await;
    let decided = scene.pending_from("alpha").await;
    let asked = scene
        .questions
        .asked(scene.launch.mob(), "alpha", "call-1")
        .await;
    // A runtime over the same mob whose first read was tried and failed.
    let unread = MobRuntime::from_handle(scene.launch.mob().handle());
    assert!(
        !unread
            .private_owners()
            .seeded(&UnreadableRecord)
            .await
            .landed(),
        "the first read has not succeeded"
    );
    let mut state = open_console(&unread);
    state.module_runtime = Some(Arc::clone(&scene.gating));
    state.durable_decisions = scene.questions.decisions.clone();
    let app = console_json_router_with_state(state);
    let mut stream = ChangeStream::open_as(&app, None).await;

    scene.evaluated_from("alpha").await;
    scene.expect(
        stream.hears("gating.changed", 20).await,
        format!("the operator's stream tells of the row: {}", stream.text),
    );
    let mut owed = Vec::new();
    for (method, params) in door_requests(&decided, &asked) {
        let answer = console_rpc_as(&app, None, method, params.clone()).await;
        scene.expect(
            !says_to_ask_again(&answer["error"]),
            format!("{method} answers the operator: {answer}"),
        );
        if matches!(method, "meta/gating/pending" | "meta/gating/decide") {
            owed.push((method, params, answer));
        }
    }
    let pending = console_rpc_as(&app, None, "meta/gating/pending", json!({})).await;
    scene.expect(
        pending["result"]["pending"]
            .as_array()
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry["pending_id"] == json!(listed))
            }),
        format!("the operator is listed a shared agent's request: {pending}"),
    );

    // The same requests once the read has landed.
    assert!(
        unread
            .private_owners()
            .seeded(&scene.launch.mob().handle())
            .await
            .landed(),
        "the first read has succeeded"
    );
    for (method, params, owed) in owed {
        let landed = console_rpc_as(&app, None, method, params).await;
        scene.expect(
            owed == landed,
            format!(
                "{method} answers the operator the same once the read has landed: {owed} then \
                 {landed}"
            ),
        );
    }
    scene.launch.finish().await;
}

/// The refusal that says to ask again is answered to a person for as long
/// as a first read keeps failing, and no longer once one has landed.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_is_told_to_ask_again_for_as_long_as_a_first_read_keeps_failing() {
    let scene = scene("origin-first-read-fails").await;
    let of_a_shared_agent = scene.pending_from("alpha").await;
    let unread = MobRuntime::from_handle(scene.launch.mob().handle());
    let app = console_with(&unread, &scene.gating, scene.questions.decisions.clone());
    for attempt in 1..=3 {
        assert!(
            !unread
                .private_owners()
                .seeded(&UnreadableRecord)
                .await
                .landed(),
            "the first read has not succeeded"
        );
        let pending = pending_on(&app, &scene.bob).await;
        scene.expect(
            pending.as_ref().is_err_and(says_to_ask_again),
            format!("after failed read {attempt} a person is told to ask again: {pending:?}"),
        );
    }
    assert!(
        unread
            .private_owners()
            .seeded(&scene.launch.mob().handle())
            .await
            .landed(),
        "the first read has succeeded"
    );
    let pending = pending_on(&app, &scene.bob).await;
    scene.expect(
        pending == Ok(set_of(&[&of_a_shared_agent])),
        format!("once a read has landed the person is listed the request: {pending:?}"),
    );
    scene.launch.finish().await;
}

/// A pin. On a Console that identifies nobody, while the first read of who
/// members are private to is owed, the rule on the name's holder now
/// decides alone: a request whose origin the Console does not know is
/// withheld from the operator, and a request an ended private member made
/// under a name a shared member holds now is listed. Once the read has
/// landed the request of the unknown origin is listed and the ended private
/// member's is not (it is its owner's and the administrator's).
#[tokio::test(flavor = "multi_thread")]
async fn on_a_console_that_identifies_nobody_the_holder_now_decides_alone_while_a_first_read_is_owed()
 {
    let scene = scene("origin-first-read-open-console-holder-now").await;
    scene.fork_the_side_chat().await;
    let of_the_side_chat = scene.pending_from(SIDE_NAME).await;
    scene.close_the_side_chat().await;
    scene.seat_shared(SIDE_NAME).await;
    let of_an_unknown_origin = scene.pending_from("a-name-no-member-holds").await;
    let unread = MobRuntime::from_handle(scene.launch.mob().handle());
    assert!(
        !unread
            .private_owners()
            .seeded(&UnreadableRecord)
            .await
            .landed(),
        "the first read has not succeeded"
    );
    let mut state = open_console(&unread);
    state.module_runtime = Some(Arc::clone(&scene.gating));
    state.durable_decisions = scene.questions.decisions.clone();
    let app = console_json_router_with_state(state);
    let listed = |answer: &Value| -> BTreeSet<String> {
        answer["result"]["pending"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry["pending_id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    let owed = console_rpc_as(&app, None, "meta/gating/pending", json!({})).await;
    scene.expect(
        listed(&owed) == set_of(&[&of_the_side_chat]),
        format!(
            "while the read is owed the operator is listed the request under the name a shared \
             member holds now, and not the one of an unknown origin: {owed}"
        ),
    );
    assert!(
        unread
            .private_owners()
            .seeded(&scene.launch.mob().handle())
            .await
            .landed(),
        "the first read has succeeded"
    );
    let landed = console_rpc_as(&app, None, "meta/gating/pending", json!({})).await;
    scene.expect(
        listed(&landed) == set_of(&[&of_an_unknown_origin]),
        format!(
            "once the read has landed the operator is listed the request of the unknown \
             origin, and not the ended private member's: {landed}"
        ),
    );
    scene.launch.finish().await;
}

/// Every signed-in person reads and decides gating and reads decisions; the
/// owner drives every agent; the other person drives the agents of the role
/// `worker` only; `ROOT` administers.
fn the_other_person_views_workers_only() -> AccessController {
    let config: crate::access::AccessControlConfig = serde_json::from_value(json!({
        "enabled": true,
        "admins": [ROOT],
        "rules": [
            {
                "id": "the-owner-drives-every-agent",
                "subjects": [ALICE],
                "actions": ["agent.*"],
                "agents": ["*"],
            },
            {
                "id": "the-other-person-drives-workers",
                "subjects": [BOB],
                "actions": ["agent.*"],
                "roles": ["worker"],
            },
            {
                "id": "everyone-reads-and-decides",
                "subjects": ["*"],
                "actions": [
                    crate::access::ACTION_GATING_VIEW,
                    crate::access::ACTION_GATING_DECIDE,
                    crate::access::ACTION_DECISIONS_VIEW,
                ],
            },
        ],
    }))
    .expect("access configuration");
    AccessController::new(config).expect("access controller")
}

/// What a shared member recorded under its name is shown to, and decided
/// by, the people who may view THAT member, when a shared member of another
/// role holds the name later. `gamma` is first a member of the role
/// `planner`, the origin of a pending request and of an audit row; it is
/// retired, and a member of the role `worker` is seated under the name and
/// is the origin of a request and an audit row of its own. The other person
/// may view workers only: they are listed, read and decide the worker's
/// entries and none of the planner's. The owner, who may view both, and the
/// administrator are listed both requests.
#[tokio::test(flavor = "multi_thread")]
async fn what_a_shared_member_recorded_stays_with_who_may_view_it_under_a_later_holder_of_another_role()
 {
    let scene = scene_under("origin-two-roles", the_other_person_views_workers_only()).await;
    let mob = scene.launch.mob();
    scene.seat_shared_as("planner", "gamma").await;
    let of_the_planner = scene.pending_from("gamma").await;
    let audit_of_the_planner = scene.evaluated_from("gamma").await;
    mob.handle()
        .retire(AgentIdentity::from("gamma"))
        .await
        .expect("retire the planner");
    scene.expect(
        scene.launch.gone_within("gamma", 10).await,
        "the roster no longer holds the planner",
    );
    tokio::time::sleep(PUMP).await;
    scene.seat_shared_as("worker", "gamma").await;
    let of_the_worker = scene.pending_from("gamma").await;
    let audit_of_the_worker = scene.evaluated_from("gamma").await;
    tokio::time::sleep(PUMP).await;

    let others_pending = scene.pending(&scene.bob).await;
    scene.expect(
        others_pending == Ok(set_of(&[&of_the_worker])),
        format!("the other person is listed the worker's request only: {others_pending:?}"),
    );
    let others_audit = scene.audit_actions(&scene.bob).await;
    scene.expect(
        others_audit.contains(&audit_of_the_worker)
            && !others_audit.contains(&audit_of_the_planner),
        format!(
            "the other person reads the worker's audit row and not the planner's: \
             {others_audit:?}"
        ),
    );
    let unknown = scene.approve(&scene.bob, "gate-pending-of-nobody").await;
    let decided = scene.approve(&scene.bob, &of_the_planner).await;
    scene.expect(
        refusal(&decided) == refusal(&unknown) && scene.still_pending(&of_the_planner).await,
        format!(
            "the other person's decide of the planner's request is answered as an id that is \
             not pending, and the request stays pending: {decided}"
        ),
    );
    for (who, bearer) in [
        ("the owner", &scene.alice),
        ("the administrator", &scene.root),
    ] {
        let pending = scene.pending(bearer).await;
        scene.expect(
            pending == Ok(set_of(&[&of_the_planner, &of_the_worker])),
            format!("{who} is listed both requests: {pending:?}"),
        );
    }
    let others_own = scene.approve(&scene.bob, &of_the_worker).await;
    scene.expect(
        others_own["error"].is_null() && !scene.still_pending(&of_the_worker).await,
        format!("the other person decides the worker's request: {others_own}"),
    );
    scene.launch.finish().await;
}

/// Every signed-in person reads and decides gating and reads decisions; the
/// owner drives every agent; the other person drives the agents labelled
/// with the team `b` only; `ROOT` administers.
fn the_other_person_views_team_b_only() -> AccessController {
    let config: crate::access::AccessControlConfig = serde_json::from_value(json!({
        "enabled": true,
        "admins": [ROOT],
        "rules": [
            {
                "id": "the-owner-drives-every-agent",
                "subjects": [ALICE],
                "actions": ["agent.*"],
                "agents": ["*"],
            },
            {
                "id": "the-other-person-drives-team-b",
                "subjects": [BOB],
                "actions": ["agent.*"],
                "match_labels": { "team": "b" },
            },
            {
                "id": "everyone-reads-and-decides",
                "subjects": ["*"],
                "actions": [
                    crate::access::ACTION_GATING_VIEW,
                    crate::access::ACTION_GATING_DECIDE,
                    crate::access::ACTION_DECISIONS_VIEW,
                ],
            },
        ],
    }))
    .expect("access configuration");
    AccessController::new(config).expect("access controller")
}

/// The spec of a shared member of the role `worker` under `name`, labelled
/// with the team `team`.
fn worker_of_team(name: &str, team: &str) -> meerkat_mob::SpawnMemberSpec {
    meerkat_mob::SpawnMemberSpec::new("worker", AgentIdentity::from(name)).with_labels(
        std::collections::BTreeMap::from([("team".to_string(), team.to_string())]),
    )
}

/// What a shared member recorded under its name before it was seated again
/// with other labels is shown to, and decided by, the people who may view
/// the member both as it was seated then and as it holds the name now; a
/// member seated again as it was keeps what it recorded with the people who
/// may view it. `gamma` is seated with the team `a` and is the origin of a
/// pending request and of an audit row; it is respawned with the team `b`
/// and is the origin of a second request and audit row; it is respawned
/// once more as it is and is the origin of a third request. The other
/// person may view the agents of the team `b` only: they are listed, read
/// and decide the second and the third request and not the first. The
/// owner, who may view every agent, and the administrator are listed all
/// three.
#[tokio::test(flavor = "multi_thread")]
async fn what_a_shared_member_recorded_before_it_was_seated_again_with_other_labels_stays_with_who_may_view_both()
 {
    let scene = scene_under("origin-seated-again", the_other_person_views_team_b_only()).await;
    let mob = scene.launch.mob();
    mob.handle()
        .spawn_spec(worker_of_team("gamma", "a"))
        .await
        .expect("seat the member of the team a");
    scene.launch.turn("gamma", "The code word is heron.").await;
    tokio::time::sleep(PUMP).await;
    let of_team_a = scene.pending_from("gamma").await;
    let audit_of_team_a = scene.evaluated_from("gamma").await;

    let relabelled = mob
        .handle()
        .respawn_with_successor_spec(worker_of_team("gamma", "b"))
        .await;
    scene.expect(
        relabelled.is_ok(),
        format!(
            "the member is seated again with the team b: {:?}",
            relabelled.as_ref().err()
        ),
    );
    tokio::time::sleep(PUMP).await;
    let of_team_b = scene.pending_from("gamma").await;
    let audit_of_team_b = scene.evaluated_from("gamma").await;

    let respawned = mob
        .handle()
        .respawn(AgentIdentity::from("gamma"), None)
        .await;
    scene.expect(
        respawned.is_ok(),
        format!(
            "the member is seated again as it is: {:?}",
            respawned.as_ref().err()
        ),
    );
    tokio::time::sleep(PUMP).await;
    let of_the_last_seating = scene.pending_from("gamma").await;
    tokio::time::sleep(PUMP).await;

    let others_pending = scene.pending(&scene.bob).await;
    scene.expect(
        others_pending == Ok(set_of(&[&of_team_b, &of_the_last_seating])),
        format!(
            "the other person is listed the requests made since the member holds the team b, \
             the one from before its last seating too, and not the one made under the team a: \
             {others_pending:?}"
        ),
    );
    let others_audit = scene.audit_actions(&scene.bob).await;
    scene.expect(
        others_audit.contains(&audit_of_team_b) && !others_audit.contains(&audit_of_team_a),
        format!(
            "the other person reads the audit row made under the team b and not the one made \
             under the team a: {others_audit:?}"
        ),
    );
    let unknown = scene.approve(&scene.bob, "gate-pending-of-nobody").await;
    let decided = scene.approve(&scene.bob, &of_team_a).await;
    scene.expect(
        refusal(&decided) == refusal(&unknown) && scene.still_pending(&of_team_a).await,
        format!(
            "the other person's decide of the request made under the team a is answered as an \
             id that is not pending, and the request stays pending: {decided}"
        ),
    );
    for (who, bearer) in [
        ("the owner", &scene.alice),
        ("the administrator", &scene.root),
    ] {
        let pending = scene.pending(bearer).await;
        scene.expect(
            pending == Ok(set_of(&[&of_team_a, &of_team_b, &of_the_last_seating])),
            format!("{who} is listed all three requests: {pending:?}"),
        );
    }
    let others_own = scene.approve(&scene.bob, &of_team_b).await;
    scene.expect(
        others_own["error"].is_null() && !scene.still_pending(&of_team_b).await,
        format!(
            "the other person decides the request made under the team b before the member's \
             last seating: {others_own}"
        ),
    );
    scene.launch.finish().await;
}
