//! What becomes of the question sets a member private to a person asked
//! once that member ends, and of the notice an ended set owes its asker.
//!
//! The launch is the gateway-shaped one of the parent module, or a mob
//! runtime with no schedule service at all. The question sets are held in
//! the durable decisions of a state directory of the row's own, through the
//! surface the members' `ask_user` reads. A row runs to its end and then
//! fails with every expectation it found unmet.

use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::decision::{DecisionId, DecisionPhase};
use meerkat_core::ask_user::{
    AskUserHost as _, AskUserOutcome, AskUserQuestionSet, AskUserRequest,
};
use meerkat_runtime::ask_user::{AskUserNotice, AskUserNotices, ask_user_result_of};

use super::*;
use crate::ask_user::{AskUserSurface, MemberAskUserHost, MemberKeyedNotices, open_question_sets};
use crate::console_schedules::{
    ConsoleScheduleDoor, ConsoleScheduleDoorSlot, ConsoleScheduledSend,
};
use crate::durable_decisions::RuntimeDecisions;
use crate::private_owners::EndedPrivateMember;

/// A Console send path that sends nothing: installed, it says the runtime
/// serves a Console on which a person can decide.
struct APersonDecides;

#[async_trait::async_trait]
impl ConsoleScheduleDoor for APersonDecides {
    fn takes_a_persons_decision(&self) -> bool {
        true
    }

    fn refuses_every_caller(&self) -> bool {
        false
    }

    async fn send(&self, _send: &ConsoleScheduledSend, _delivery_key: &str) -> Result<(), String> {
        Ok(())
    }
}

/// The notices the executor handed over, each settled as failed for good:
/// what a notice for a name no member holds settles as.
#[derive(Default)]
struct Told(Mutex<Vec<AskUserNotice>>);

impl Told {
    fn outcome_of(&self, pending_id: &str) -> Option<AskUserOutcome> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|notice| notice.decision_id.as_str() == pending_id)
            .map(|notice| notice.result.outcome)
    }
}

#[async_trait::async_trait]
impl AskUserNotices for Told {
    async fn deliver(&self, notice: &AskUserNotice) -> Option<ApprovalConsentDeliveryState> {
        self.0.lock().unwrap().push(notice.clone());
        Some(ApprovalConsentDeliveryState::Failed {
            reason: "the asker is no longer an agent of the mob".to_string(),
        })
    }
}

/// The question sets of a runtime's members: the durable decisions they are
/// held in, and what keeps their surface served.
struct Questions {
    decisions: RuntimeDecisions,
    mob_id: String,
    state_dir: tempfile::TempDir,
    doors: ConsoleScheduleDoorSlot,
}

/// Serve the questions the members of `mob` ask from the durable decisions
/// of a new state directory, on a Console on which a person can decide.
fn serve_questions(mob: &MobRuntime) -> Questions {
    let mob_id = mob.handle().mob_id().to_string();
    let state_dir = tempfile::tempdir().expect("state directory");
    let decisions = RuntimeDecisions::in_state_dir(
        state_dir.path(),
        meerkat_core::RealmId::parse("team").expect("realm"),
        crate::durable_decisions::DecisionIngressDeclarations::default(),
        Arc::new(
            crate::runtime::SqliteMetadataStore::open(state_dir.path().join("metadata.sqlite3"))
                .expect("metadata store"),
        ),
        mob_id.clone(),
        true,
    );
    let doors = ConsoleScheduleDoorSlot::default();
    doors.install(Arc::new(APersonDecides));
    let questions = Questions {
        decisions,
        mob_id,
        state_dir,
        doors,
    };
    questions.serve(mob);
    questions
}

fn question_set() -> AskUserQuestionSet {
    serde_json::from_value(json!({
        "title": "Release plan",
        "questions": [{
            "id": "q1",
            "prompt": "Which day?",
            "choices": [{"id": "a", "label": "Monday"}, {"id": "b", "label": "Friday"}],
        }],
    }))
    .expect("question set")
}

impl Questions {
    /// Fill the surface the members' `ask_user` reads over these decisions:
    /// what a runtime does when it starts serving its members' questions.
    fn serve(&self, mob: &MobRuntime) {
        mob.ask_user_slot().fill(AskUserSurface::new(
            self.decisions.clone(),
            self.doors.served(),
            self.mob_id.clone(),
        ));
    }

    /// The member `asker` of `mob` asks the row's question set in the call
    /// `call_id`: the pending id of the set it holds.
    async fn asked(&self, mob: &MobRuntime, asker: &str, call_id: &str) -> String {
        let host = MemberAskUserHost::new(
            mob.ask_user_slot().clone(),
            self.mob_id.clone(),
            asker.to_string(),
        );
        let result = host
            .ask(AskUserRequest {
                questions: &question_set(),
                timeout: Duration::from_hours(1),
                run_id: &meerkat_core::RunId::new(),
                call_id,
            })
            .await
            .expect("the host takes the call");
        assert_eq!(result.outcome, AskUserOutcome::Asked, "{result:?}");
        result.request_id.expect("an asked set has a request id")
    }

    fn row(&self, pending_id: &str) -> meerkat_core::approval::decision::StoredDecision {
        self.decisions
            .owner()
            .get(&DecisionId::new(pending_id.to_string()).expect("decision id"))
            .expect("decision row")
    }

    /// The phase the decision store holds the set in now.
    fn phase(&self, pending_id: &str) -> DecisionPhase {
        self.row(pending_id).state.phase
    }

    /// Whether the store holds the set as withdrawn within `seconds`.
    async fn withdrawn_within(&self, pending_id: &str, seconds: u64) -> bool {
        for _ in 0..seconds * 10 {
            if self.phase(pending_id) == DecisionPhase::Withdrawn {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// The pending ids of the sets a read of the open sets lists, sorted.
    fn open(&self) -> Vec<String> {
        let mut open: Vec<String> = open_question_sets(&self.decisions)
            .expect("the open sets")
            .iter()
            .map(|set| set.decision_id().to_string())
            .collect();
        open.sort();
        open
    }

    /// The files of the decision store in the row's state directory.
    fn store_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.state_dir.path())
            .expect("read the state directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.starts_with("approval_decisions"))
            .collect();
        names.sort();
        names
    }

    /// The notice the set owes its asker now, as the executor hands it over.
    fn notice(&self, pending_id: &str, asker: &str) -> AskUserNotice {
        let row = self.row(pending_id);
        AskUserNotice {
            decision_id: row.decision_id.clone(),
            asker: asker.to_string(),
            result: ask_user_result_of(&row).expect("the set's result"),
        }
    }
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// The console with sign-in over `launch` and a gating owner of its own: a
/// person decides a question set on it.
fn deciding_console(launch: &Launch) -> Router {
    let owner = crate::start_meta_runtime(
        crate::MetaConfig {
            modules: vec![],
            discovery: crate::DiscoverySpec {
                namespace: format!("{}-gating", launch.mob_id),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        Duration::from_secs(1),
    )
    .expect("gating owner");
    let mut state = enforced_console(launch.mob(), None);
    state.module_runtime = Some(Arc::new(tokio::sync::Mutex::new(owner)));
    console_json_router_with_state(state)
}

/// The person of `bearer` answers the set `pending_id` on `app`.
async fn answer(app: &Router, bearer: &str, questions: &Questions, pending_id: &str) -> Value {
    console_rpc_as(
        app,
        Some(bearer),
        "meta/gating/decide",
        json!({
            "pending_id": pending_id,
            "decision": "approve",
            "presented_digest": questions.row(pending_id).descriptor.subject_digest.to_string(),
            "answers": [{"question_id": "q1", "answer": {"kind": "choice", "choice_id": "a"}}],
        }),
    )
    .await
}

/// A person closes her private side chat while a question it asked is open:
/// the set is withdrawn, by the decision's own lifecycle, and is no longer
/// among the open sets. The open set of a shared member stays as it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chats_open_question_set_is_withdrawn_when_its_owner_closes_it() {
    let launch = boot("questions-close").await;
    let (app, owner) = private_side_chat(&launch).await;
    let questions = serve_questions(launch.mob());
    let of_the_side_chat = questions.asked(launch.mob(), SIDE_NAME, "call-1").await;
    let of_a_shared_member = questions.asked(launch.mob(), "alpha", "call-1").await;
    launch.expect(
        questions.open() == sorted(vec![of_the_side_chat.clone(), of_a_shared_member.clone()]),
        "both sets are open while the private side chat lives",
    );

    close(&launch, &app, &owner, SIDE_NAME).await;
    launch.expect(
        questions.withdrawn_within(&of_the_side_chat, 10).await,
        format!(
            "the closed private side chat's set is withdrawn (it is {:?})",
            questions.phase(&of_the_side_chat)
        ),
    );
    launch.expect(
        questions.phase(&of_a_shared_member) == DecisionPhase::Open,
        format!(
            "a shared member's set stays open (it is {:?})",
            questions.phase(&of_a_shared_member)
        ),
    );
    launch.expect(
        questions.open() == vec![of_a_shared_member.clone()],
        format!(
            "only the shared member's set is among the open sets: {:?}",
            questions.open()
        ),
    );
    launch.finish().await;
}

/// An ephemeral member private to a person that is no side chat, retired by
/// its host: it ends, and the open set it asked is withdrawn.
#[tokio::test(flavor = "multi_thread")]
async fn an_ephemeral_private_members_open_question_set_is_withdrawn_when_the_member_is_retired() {
    let launch = boot("questions-retired").await;
    let name = "notes";
    launch
        .mob()
        .handle()
        .spawn_spec(private_member_spec("planner", name, false))
        .await
        .expect("seat a member private to a person");
    let questions = serve_questions(launch.mob());
    let asked = questions.asked(launch.mob(), name, "call-1").await;
    launch
        .mob()
        .handle()
        .retire(AgentIdentity::from(name))
        .await
        .expect("retire the private member");
    launch.expect(
        launch.gone_within(name, 10).await,
        "the roster no longer holds the retired member",
    );
    launch.expect(
        questions.withdrawn_within(&asked, 10).await,
        format!(
            "the retired private member's set is withdrawn (it is {:?})",
            questions.phase(&asked)
        ),
    );
    launch.finish().await;
}

/// A respawn of a private side chat seats the next member under its name:
/// it is no end, and the set the side chat asked stays open.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawned_private_side_chat_keeps_its_open_question_set() {
    let launch = boot("questions-respawn").await;
    let (_app, _owner) = private_side_chat(&launch).await;
    let questions = serve_questions(launch.mob());
    let asked = questions.asked(launch.mob(), SIDE_NAME, "call-1").await;
    let respawned = launch
        .mob()
        .handle()
        .respawn(AgentIdentity::from(SIDE_NAME), None)
        .await;
    launch.expect(
        respawned.is_ok(),
        format!(
            "the private side chat is respawned: {:?}",
            respawned.as_ref().err()
        ),
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    launch.expect(
        questions.phase(&asked) == DecisionPhase::Open,
        format!(
            "a respawned private side chat's set stays open (it is {:?})",
            questions.phase(&asked)
        ),
    );
    launch.finish().await;
}

/// The end of a member private to a person, handed to the followers of the
/// launch: every open set that member asked before its end is withdrawn,
/// and its asker is owed one `withdrawn` notice for each. A set of another
/// member, a set asked under the same name after the end, and a set that
/// had ended before stay as they are; so does everything when the same end
/// is handed over again. The member is named as its roster names it: a name
/// its roster id only spells is the member's too.
#[tokio::test(flavor = "multi_thread")]
async fn the_end_of_a_private_member_withdraws_the_open_sets_it_asked_before_its_end() {
    use DecisionPhase::{Open, Withdrawn};

    let launch = boot("questions-rule").await;
    let mob = launch.mob();
    let questions = serve_questions(mob);
    let told = Arc::new(Told::default());
    let first = questions.asked(mob, "notes", "call-1").await;
    let second = questions.asked(mob, "notes", "call-2").await;
    let by_spelled_name = questions.asked(mob, "desk:front", "call-1").await;
    let of_another_member = questions.asked(mob, "scout", "call-1").await;
    let of_another_spelled_name = questions.asked(mob, "desk:back", "call-1").await;
    // A set of the member that the host ended before the member did.
    let ended_before = questions.asked(mob, "notes", "call-3").await;
    let ended = questions.decisions.owner().withdraw(
        &DecisionId::new(ended_before.clone()).expect("decision id"),
        &meerkat_runtime::decision::DecisionWithdrawer::Host,
    );
    launch.expect(ended.is_ok(), "the host withdraws a set before the end");
    // The executor's driver, armed before the end: it passes at its start
    // and then only when it is told that a decision changed.
    let driver = crate::ask_user::spawn_ask_user_driver(
        &questions.decisions,
        Arc::clone(&told) as Arc<dyn AskUserNotices>,
        Duration::from_hours(1),
        mob.ask_user_slot()
            .held_signal()
            .expect("the surface is filled"),
    )
    .expect("the driver is armed");
    for _ in 0..100 {
        if told.outcome_of(&ended_before).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    launch.expect(
        told.outcome_of(&ended_before) == Some(AskUserOutcome::Withdrawn),
        "the driver's first pass settles the set that ended before",
    );

    tokio::time::sleep(Duration::from_millis(30)).await;
    let ended_at = chrono::Utc::now();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let asked_after_the_end = questions.asked(mob, "notes", "call-4").await;

    let ends = [
        EndedPrivateMember {
            mob_id: launch.mob_id.clone(),
            member: "notes".to_string(),
            session: None,
            ended_at,
        },
        EndedPrivateMember {
            mob_id: launch.mob_id.clone(),
            member: crate::member_comms_id::mob_member_id_str("desk:front").into_owned(),
            session: None,
            ended_at,
        },
    ];
    for pass in ["the end", "the end handed over again"] {
        for end in &ends {
            let taken = mob.private_owners().member_ended(end.clone()).await;
            launch.expect(taken, format!("{pass}: the followers take the end"));
        }
        for (what, pending_id, expected) in [
            ("the member's first set", &first, Withdrawn),
            ("the member's second set", &second, Withdrawn),
            ("the set of the spelled name", &by_spelled_name, Withdrawn),
            ("another member's set", &of_another_member, Open),
            ("another spelled name's set", &of_another_spelled_name, Open),
            ("a set asked after the end", &asked_after_the_end, Open),
            ("a set that ended before", &ended_before, Withdrawn),
        ] {
            let phase = questions.phase(pending_id);
            launch.expect(
                phase == expected,
                format!("{pass}: {what} is {phase:?}, expected {expected:?}"),
            );
        }
        launch.expect(
            questions.open()
                == sorted(vec![
                    of_another_member.clone(),
                    of_another_spelled_name.clone(),
                    asked_after_the_end.clone(),
                ]),
            format!("{pass}: the open sets are {:?}", questions.open()),
        );
    }
    // The withdrawal tells the driver that a decision changed: each
    // withdrawn set owes its asker one notice.
    for _ in 0..100 {
        if [&first, &second, &by_spelled_name]
            .iter()
            .all(|pending_id| told.outcome_of(pending_id).is_some())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for (what, pending_id) in [
        ("the member's first set", &first),
        ("the member's second set", &second),
        ("the set of the spelled name", &by_spelled_name),
    ] {
        launch.expect(
            told.outcome_of(pending_id) == Some(AskUserOutcome::Withdrawn),
            format!(
                "the asker of {what} is owed a withdrawn notice: {:?}",
                told.outcome_of(pending_id)
            ),
        );
    }
    launch.expect(
        told.outcome_of(&of_another_member).is_none()
            && told.outcome_of(&asked_after_the_end).is_none(),
        "no notice is owed for a set that stays open",
    );
    driver.shutdown().await;
    launch.finish().await;
}

/// The end of a private member in a runtime that never held a question set:
/// the followers take the end, nothing is withdrawn, and no file of the
/// decision store is created.
#[tokio::test(flavor = "multi_thread")]
async fn the_end_of_a_private_member_that_asked_nothing_opens_no_decision_store() {
    let launch = boot("questions-none").await;
    let questions = serve_questions(launch.mob());
    let taken = launch
        .mob()
        .private_owners()
        .member_ended(EndedPrivateMember {
            mob_id: launch.mob_id.clone(),
            member: "notes".to_string(),
            session: None,
            ended_at: chrono::Utc::now(),
        })
        .await;
    launch.expect(taken, "the followers take the end");
    launch.expect(
        questions.store_files().is_empty(),
        format!(
            "no file of the decision store is created: {:?}",
            questions.store_files()
        ),
    );
    launch.expect(questions.open().is_empty(), "no set is open");
    launch.finish().await;
}

/// A mob runtime with no schedule service: nothing adopts schedules there,
/// and the end of a private member withdraws the open set it asked all the
/// same.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_with_no_schedule_service_withdraws_an_ended_private_members_question_set() {
    let (_directory, runtime) =
        super::super::tests::build_empty_console_test_runtime("questions-no-schedules")
            .await
            .expect("mob runtime");
    let questions = serve_questions(&runtime);
    let asked = questions.asked(&runtime, "notes", "call-1").await;
    let of_another_member = questions.asked(&runtime, "scout", "call-1").await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    runtime
        .private_owners()
        .member_ended(EndedPrivateMember {
            mob_id: questions.mob_id.clone(),
            member: "notes".to_string(),
            session: None,
            ended_at: chrono::Utc::now(),
        })
        .await;
    let withdrawn = questions.withdrawn_within(&asked, 5).await;
    let others = questions.phase(&of_another_member);
    let _ = runtime.handle().stop().await;

    assert!(
        withdrawn,
        "the ended private member's set is withdrawn (it is {:?})",
        questions.phase(&asked)
    );
    assert_eq!(others, DecisionPhase::Open, "another member's set");
}

/// A set a private member left open because nothing acted on its end (the
/// runtime served no questions when the member ended) is withdrawn when the
/// runtime serves its members' questions again, once the journal's record
/// of the end is read. The open set of a shared member that ended, and the
/// open set of a living member, stay as they are.
#[tokio::test(flavor = "multi_thread")]
async fn serving_questions_again_withdraws_the_open_sets_of_private_members_that_ended() {
    let launch = boot("questions-served-again").await;
    let mob = launch.mob();
    let name = "notes";
    mob.handle()
        .spawn_spec(private_member_spec("planner", name, false))
        .await
        .expect("seat a member private to a person");
    launch.shared("worker", "beta").await;
    launch.shared("worker", "gamma").await;
    let questions = serve_questions(mob);
    let of_the_private_member = questions.asked(mob, name, "call-1").await;
    let of_an_ended_shared_member = questions.asked(mob, "beta", "call-1").await;
    let of_a_living_member = questions.asked(mob, "gamma", "call-1").await;

    // The runtime stops serving its members' questions; both members end.
    mob.ask_user_slot().clear();
    for member in [name, "beta"] {
        mob.handle()
            .retire(AgentIdentity::from(member))
            .await
            .expect("retire the member");
        launch.expect(
            launch.gone_within(member, 10).await,
            format!("the roster no longer holds {member}"),
        );
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    launch.expect(
        questions.phase(&of_the_private_member) == DecisionPhase::Open,
        "the set is open while the runtime serves no questions",
    );

    questions.serve(mob);
    launch.expect(
        questions.withdrawn_within(&of_the_private_member, 10).await,
        format!(
            "the ended private member's set is withdrawn once questions are served again (it is \
             {:?})",
            questions.phase(&of_the_private_member)
        ),
    );
    for (what, pending_id) in [
        ("an ended shared member's set", &of_an_ended_shared_member),
        ("a living member's set", &of_a_living_member),
    ] {
        launch.expect(
            questions.phase(pending_id) == DecisionPhase::Open,
            format!(
                "{what} stays open (it is {:?})",
                questions.phase(pending_id)
            ),
        );
    }
    launch.finish().await;
}

/// The notice an ended set owes goes to the member that asked, and to no
/// later holder of its name. A private side chat asks, its owner answers,
/// and the side chat is closed before the notice is taken; a shared member
/// is then seated under the name. The notice is settled as failed for good:
/// it is not left to be asked again for whoever holds the name. The notice
/// of a living member's answered set is not settled so: it stays owed to
/// that member.
#[tokio::test(flavor = "multi_thread")]
async fn the_notice_of_an_ended_private_members_set_reaches_no_later_holder_of_its_name() {
    let launch = boot("questions-notice").await;
    let mob = launch.mob();
    let (side_chat_app, owner) = private_side_chat(&launch).await;
    let app = deciding_console(&launch);
    let questions = serve_questions(mob);
    let of_the_side_chat = questions.asked(mob, SIDE_NAME, "call-1").await;
    let of_a_living_member = questions.asked(mob, "alpha", "call-1").await;
    for (what, pending_id) in [
        ("the side chat's set", &of_the_side_chat),
        ("the living member's set", &of_a_living_member),
    ] {
        let answered = answer(&app, &owner, &questions, pending_id).await;
        launch.expect(
            answered["result"]["outcome"] == json!("answered"),
            format!("the owner answers {what}: {answered}"),
        );
    }

    // No executor has taken the notices. The side chat ends, and a shared
    // member is seated under its name.
    close(&launch, &side_chat_app, &owner, SIDE_NAME).await;
    launch.shared("worker", SIDE_NAME).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let notices = MemberKeyedNotices::new(mob.clone());
    let to_the_name = notices
        .deliver(&questions.notice(&of_the_side_chat, SIDE_NAME))
        .await;
    launch.expect(
        matches!(
            to_the_name,
            Some(ApprovalConsentDeliveryState::Failed { .. })
        ),
        format!(
            "the notice of the ended side chat's set is settled as failed for good: \
             {to_the_name:?}"
        ),
    );
    let to_the_living_member = notices
        .deliver(&questions.notice(&of_a_living_member, "alpha"))
        .await;
    launch.expect(
        to_the_living_member.is_none(),
        format!(
            "the notice of a living member's set stays owed to it (this launch has no identity \
             runtime to send it through): {to_the_living_member:?}"
        ),
    );
    launch.finish().await;
}

/// The names under which a runtime keeps a gating record: the origin of a
/// pending approval and of an audit row, and the asking member of a
/// question set, open or ended, each as a member's memory scope spells it.
/// An approval with no origin names nobody.
#[tokio::test(flavor = "multi_thread")]
async fn the_names_with_a_gating_record_are_the_origins_and_the_askers() {
    let launch = boot("questions-names").await;
    let mob = launch.mob();
    let questions = serve_questions(mob);
    let _open = questions.asked(mob, "notes", "call-1").await;
    let ended = questions.asked(mob, "desk:front", "call-1").await;
    let withdrawn = questions.decisions.owner().withdraw(
        &DecisionId::new(ended).expect("decision id"),
        &meerkat_runtime::decision::DecisionWithdrawer::Host,
    );
    launch.expect(withdrawn.is_ok(), "the host withdraws a set");
    for (risk_tier, origin) in [
        ("r3", Some("origin-of-a-pending-request")),
        ("r1", Some("origin-of-an-audit-row")),
        ("r3", None),
    ] {
        let request = json!({
            "action": "Publish the reviewed report",
            "actor_id": "agent:writer",
            "risk_tier": risk_tier,
            "approval_timeout_ms": 600_000,
        });
        launch
            .runtime
            .evaluate_gating_action_with_origin(
                serde_json::from_value(request).expect("gating request"),
                origin.map(|identity| crate::runtime::GatingOrigin {
                    mob_id: None,
                    identity: identity.to_string(),
                    conversation_id: None,
                    interaction_id: None,
                }),
            )
            .await;
    }
    let names = launch.runtime.names_with_gating_records().await;
    launch.expect(
        names.as_ref().ok()
            == Some(
                &[
                    "desk:front",
                    "notes",
                    "origin-of-a-pending-request",
                    "origin-of-an-audit-row",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ),
        format!("the names with a gating record: {names:?}"),
    );
    launch.finish().await;
}

/// The end of a member private to a person withdraws the open sets it
/// asked whatever time the journal gave that end: a set asked while the
/// member last seated under the name held it is that member's, also one
/// whose time is later than the end's (the clock that stamps both was set
/// back in between). The name had an earlier holder, so the ended member
/// is the one last seated under it, not the only one.
#[tokio::test(flavor = "multi_thread")]
async fn the_end_of_the_member_last_seated_under_a_name_withdraws_its_sets_whatever_the_ends_time()
{
    let launch = boot("questions-end-time").await;
    let mob = launch.mob();
    let questions = serve_questions(mob);
    // An earlier holder of the name, private to no one, is retired.
    launch.shared("worker", SIDE_NAME).await;
    mob.handle()
        .retire(AgentIdentity::from(SIDE_NAME))
        .await
        .expect("retire the earlier holder");
    launch.expect(
        launch.gone_within(SIDE_NAME, 10).await,
        "the roster no longer holds the earlier holder",
    );
    let (_app, _owner) = private_side_chat(&launch).await;
    // The launch's event pump shows the record the seatings.
    tokio::time::sleep(Duration::from_secs(2)).await;
    // The time the journal gives the side chat's end: after its seating,
    // before its set's.
    let ended_at = chrono::Utc::now();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let of_the_side_chat = questions.asked(mob, SIDE_NAME, "call-1").await;

    // The side chat's end, with a journal time before its set's.
    let taken = mob
        .private_owners()
        .member_ended(EndedPrivateMember {
            mob_id: launch.mob_id.clone(),
            member: SIDE_NAME.to_string(),
            session: None,
            ended_at,
        })
        .await;
    launch.expect(taken, "the followers take the end");
    launch.expect(
        questions.withdrawn_within(&of_the_side_chat, 10).await,
        "the set the side chat asked is withdrawn, though its time is later than the end's",
    );
    launch.finish().await;
}

/// A third person's reads of the decision `decision_id` through the
/// decisions family, each expected to answer that no decision has this id.
async fn a_third_person_reads_no_decision(
    launch: &Launch,
    app: &Router,
    third: &str,
    decision_id: &str,
    when: &str,
) {
    for method in ["meta/decisions/get", "meta/decisions/rendering"] {
        let answer = console_rpc_as(
            app,
            Some(third),
            method,
            json!({ "decision_id": decision_id }),
        )
        .await;
        launch.expect(
            answer["error"]["message"] == json!("no decision has this id"),
            format!(
                "{when}: {method} answers a third person that no decision has this id: {answer}"
            ),
        );
    }
}

/// A control. On a Console with sign-in and no access configuration, the
/// decisions family answers a third person, for the id of a question set a
/// private side chat asked, that no decision has this id: while the side
/// chat lives, after its owner closed it, and once a shared member holds
/// its name.
#[tokio::test(flavor = "multi_thread")]
async fn the_decisions_family_tells_a_third_person_that_no_decision_has_a_private_members_sets_id()
{
    let launch = boot("questions-decisions-family").await;
    launch.shared("planner", "alpha").await;
    let questions = serve_questions(launch.mob());
    let mut state = enforced_console(launch.mob(), None);
    state.durable_decisions = questions.decisions.clone();
    let app = console_json_router_with_state(state);
    let owner = bearer(alice_claims());
    let third = bearer(super::super::console_private_side_chat_tests::bob_claims());
    let forked = fork_on_console(&app, Some(&owner), "alpha", SIDE_NAME).await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let asked = questions.asked(launch.mob(), SIDE_NAME, "call-1").await;

    a_third_person_reads_no_decision(&launch, &app, &third, &asked, "while the side chat lives")
        .await;
    close(&launch, &app, &owner, SIDE_NAME).await;
    a_third_person_reads_no_decision(&launch, &app, &third, &asked, "after it was closed").await;
    launch.shared("worker", SIDE_NAME).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    a_third_person_reads_no_decision(
        &launch,
        &app,
        &third,
        &asked,
        "once a shared member holds its name",
    )
    .await;
    launch.finish().await;
}

/// The end of a member private to a person withdraws an open set asked
/// under its name at or before the end's time, whoever the seatings say
/// held the name when it was asked: a set's time can be earlier than its
/// member's seating only because the clock that stamps both was set back in
/// between. So a set an earlier holder of the name left open, asked before
/// the private member's end, is withdrawn at that end too.
#[tokio::test(flavor = "multi_thread")]
async fn the_end_of_a_private_member_withdraws_every_open_set_asked_under_its_name_before_its_end()
{
    let launch = boot("questions-end-bound").await;
    let mob = launch.mob();
    let questions = serve_questions(mob);
    // An earlier holder of the name, private to no one, asks and is retired.
    launch.shared("worker", SIDE_NAME).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let asked_in_an_earlier_seating = questions.asked(mob, SIDE_NAME, "call-0").await;
    mob.handle()
        .retire(AgentIdentity::from(SIDE_NAME))
        .await
        .expect("retire the earlier holder");
    launch.expect(
        launch.gone_within(SIDE_NAME, 10).await,
        "the roster no longer holds the earlier holder",
    );
    let (_app, _owner) = private_side_chat(&launch).await;
    // The launch's event pump shows the record the seatings.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let of_another_member = questions.asked(mob, "alpha", "call-1").await;

    // The side chat's end, at its own time.
    let taken = mob
        .private_owners()
        .member_ended(EndedPrivateMember {
            mob_id: launch.mob_id.clone(),
            member: SIDE_NAME.to_string(),
            session: None,
            ended_at: chrono::Utc::now(),
        })
        .await;
    launch.expect(taken, "the followers take the end");
    launch.expect(
        questions
            .withdrawn_within(&asked_in_an_earlier_seating, 10)
            .await,
        "the set asked under the name before the end is withdrawn, though the seatings say an \
         earlier holder held the name then",
    );
    launch.expect(
        questions.phase(&of_another_member) == DecisionPhase::Open,
        "another member's set stays open",
    );
    launch.finish().await;
}

#[path = "console_gating_origin_time_tests.rs"]
mod gating_origin_time;

/// The end of a member private to a person, followed late, leaves the open
/// sets of a later member under its name as they are. The owner closes her
/// side chat and forks a new one under the same name, which asks; then the
/// first side chat's end is handed to the followers again, with its own
/// time: the living side chat's set stays open.
#[tokio::test(flavor = "multi_thread")]
async fn an_end_followed_late_leaves_the_open_sets_of_a_later_member_under_the_name() {
    let launch = boot("questions-late-end").await;
    let mob = launch.mob();
    let questions = serve_questions(mob);
    let (app, owner) = private_side_chat(&launch).await;
    close(&launch, &app, &owner, SIDE_NAME).await;
    let ended_at = chrono::Utc::now();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let forked = fork_on_console(&app, Some(&owner), "alpha", SIDE_NAME).await;
    assert!(forked["error"].is_null(), "{forked:#}");
    // The launch's event pump shows the record the seatings and the end.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let of_the_later_side_chat = questions.asked(mob, SIDE_NAME, "call-1").await;

    let taken = mob
        .private_owners()
        .member_ended(EndedPrivateMember {
            mob_id: launch.mob_id.clone(),
            member: SIDE_NAME.to_string(),
            session: None,
            ended_at,
        })
        .await;
    launch.expect(taken, "the followers take the end");
    // The withdrawal of a set is made within the hand-over's own passes.
    tokio::time::sleep(Duration::from_secs(3)).await;
    launch.expect(
        questions.phase(&of_the_later_side_chat) == DecisionPhase::Open,
        "the living side chat's set stays open",
    );
    launch.finish().await;
}
