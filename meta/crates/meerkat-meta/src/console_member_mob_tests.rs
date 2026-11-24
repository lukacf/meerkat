//! Rows for a request that names a member by two values on the console's
//! request door.
//!
//! A row of the console names its door in `mob_id`, the mob its member sits
//! in in `member_mob_id` and the member's name there in `member_name`. A
//! request names the member with those values as they stand: `mob_id` as
//! before, `member_mob_id`, and the name in the field the method reads its
//! member from. With `member_mob_id` the name is the member's name in that
//! mob and is read as it is written. The one string a door knows a member
//! by (`<its mob>/<its name>` for a member of a mob nested under the door's
//! mob) still names it, unless two members the method would answer the
//! caller for spell it: then the request is refused and each of the two is
//! named by its two values.
//!
//! The fixture is a launch mob with the member `alpha`, whose side chat
//! `alpha-side`, forked by the signed-in owner, created a mob with the
//! member `sub-helper`: the owner owns the side chat, the mob and its
//! member, and another signed-in person is answered for none of them. A
//! row can seat in the launch mob a member whose name is the one string the
//! door knows that nested member by: any signed-in person is answered for
//! it.
//!
//! Not covered by this file: the row of a nested mob that the gateway
//! knows and has not loaded. A request by two values reads loaded mobs
//! only, so its owner is answered as for a mob the host does not have. No
//! fixture of this file brings a nested mob into that state; the row is
//! written on a launch over a declared store, which loads a mob on demand.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_mob::{AgentIdentity, MobDefinition, MobRuntimeMode, SpawnMemberSpec};
use serde_json::{Value, json};

use super::console_private_side_chat_tests::{
    alice_claims, bearer, bob_claims, enforced_console, fork_on_console, send_on_console,
};
use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
use super::*;
use crate::console_aggregator::HideImplicitDelegateMembersConsoleVisibilityPolicy;
use crate::mob_handle_runtime::MobRuntime;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The member of the launch mob, and the member of the mob its side chat
/// creates.
const LAUNCH_MEMBER: &str = "alpha";
const NESTED_MEMBER: &str = "sub-helper";

/// The read methods that name one member, each with the field it reads its
/// member from.
const READS_OF_ONE_MEMBER: &[(&str, &str)] = &[
    ("meta/console/inspect_identity", "identity"),
    ("meta/console/query_timeline", "identity"),
    ("meta/inspect_identity", "identity"),
    ("meta/status_identity", "identity"),
    ("meta/get_member", "member_id"),
    ("meta/member_status", "member_id"),
];

/// Those of them that answer a member of a mob nested under the door's mob
/// with a record of that member: the same record whichever form the
/// request names the member by.
const READS_THAT_ANSWER_A_RECORD: &[(&str, &str)] = &[
    ("meta/console/inspect_identity", "identity"),
    ("meta/inspect_identity", "identity"),
    ("meta/status_identity", "identity"),
];

/// The reads of the roster of the door's own mob: they name a member of
/// that mob by its name there and reach no member of a mob nested under
/// it, by either form.
const ROSTER_READS: &[(&str, &str)] = &[
    ("meta/get_member", "member_id"),
    ("meta/member_status", "member_id"),
];

/// The reads that answer a member of the launch mob with bytes that stand
/// still between two equal requests, apart from the progress a status
/// reports.
const STEADY_READS_OF_A_LAUNCH_MEMBER: &[(&str, &str)] = &[
    ("meta/console/inspect_identity", "identity"),
    ("meta/inspect_identity", "identity"),
    ("meta/status_identity", "identity"),
    ("meta/get_member", "member_id"),
    ("meta/member_status", "member_id"),
];

/// The reads that answer a page for the member they name. The fixture's
/// console serves no schedules: the schedule methods are held by the
/// schedule rows.
const READS_THAT_ANSWER_A_LIST: &[(&str, &str)] = &[("meta/console/query_timeline", "identity")];

/// A console over the fixture's launch mob.
struct Nested {
    app: Router,
    runtime: MobRuntime,
    /// The bearer of the person who owns the nested mob.
    owner: String,
    /// The bearer of another signed-in person.
    other: String,
    /// The launch mob's id.
    launch_mob: String,
    /// The id of the mob the side chat creates.
    team: String,
    _state: tempfile::TempDir,
}

impl Nested {
    /// The fixture over a launch mob named `tag`, with the nested mob.
    async fn start(tag: &str) -> TestResult<Self> {
        let nested = Self::start_without_the_nested_mob(tag).await?;
        nested.create_the_nested_mob().await?;
        Ok(nested)
    }

    /// The fixture over a launch mob named `tag`: the launch mob's member,
    /// and no side chat and no nested mob yet.
    async fn start_without_the_nested_mob(tag: &str) -> TestResult<Self> {
        let state = tempfile::tempdir()?;
        let runtime = persistent_gateway_runtime_in(state.path(), tag, "", "").await?;
        seeded_member(&runtime, "worker", LAUNCH_MEMBER, &[]).await?;
        Ok(Self {
            app: Self::console_of(&runtime),
            launch_mob: runtime.mob_id().to_string(),
            runtime,
            owner: bearer(alice_claims()),
            other: bearer(bob_claims()),
            team: format!("{tag}-team"),
            _state: state,
        })
    }

    /// The owner forks the side chat, and the side chat creates the nested
    /// mob with its member.
    async fn create_the_nested_mob(&self) -> TestResult {
        let forked =
            fork_on_console(&self.app, Some(&self.owner), LAUNCH_MEMBER, "alpha-side").await;
        if !forked["error"].is_null() {
            return Err(format!("the side chat is not forked: {forked}").into());
        }
        let side_chat = AgentIdentity::from("alpha-side");
        self.runtime.handle().ensure_member_live(&side_chat).await?;
        let side_session = self
            .runtime
            .handle()
            .get_member(&side_chat)
            .await?
            .and_then(|entry| entry.bridge_session_id().cloned())
            .ok_or("the side chat has a session")?;
        let mobs = self
            .runtime
            .agent_mob_mcp_state()
            .ok_or("the runtime carries agent mob tools")?;
        let definition: MobDefinition = serde_json::from_value(json!({
            "id": self.team,
            "profiles": {
                "worker": {
                    "model": "gpt-5.5",
                    "runtime_mode": "turn_driven",
                    "tools": { "comms": true }
                }
            }
        }))?;
        let created = Box::pin(mobs.mob_create_definition_with_owner_bridge_session(
            definition,
            side_session.clone(),
            true,
            false,
        ))
        .await
        .map_err(|error| error.to_string())?;
        let handle = Box::pin(mobs.handle_for(&created))
            .await
            .map_err(|error| error.to_string())?;
        Box::pin(
            handle.spawn_spec_with_generated_owner_context(
                SpawnMemberSpec::new("worker", AgentIdentity::from(NESTED_MEMBER))
                    .with_runtime_mode(MobRuntimeMode::TurnDriven),
                side_session,
            ),
        )
        .await?;
        Ok(())
    }

    /// The one string the launch mob's door knows the nested member by.
    fn nested_by_one_string(&self) -> String {
        format!("{}/{NESTED_MEMBER}", self.team)
    }

    /// Seat in the launch mob a member whose name is the one string the
    /// door knows the nested member by. The host seats it: it is no
    /// person's own.
    async fn seat_the_namesake(&self) -> TestResult {
        let one_string = self.nested_by_one_string();
        let name = crate::member_comms_id::mob_member_id_str(&one_string);
        self.runtime
            .handle()
            .ensure_member(
                SpawnMemberSpec::new("worker", AgentIdentity::from(name.as_ref()))
                    .with_runtime_mode(MobRuntimeMode::TurnDriven),
            )
            .await?;
        Ok(())
    }

    /// The holder of `bearer` forks a side chat of the launch mob's member
    /// under the one string the door knows the nested member by: a member
    /// of the launch mob of that name that is that person's own.
    async fn fork_the_namesake_as(&self, bearer: &str) -> TestResult {
        let forked = fork_on_console(
            &self.app,
            Some(bearer),
            LAUNCH_MEMBER,
            &self.nested_by_one_string(),
        )
        .await;
        if !forked["error"].is_null() {
            return Err(format!("the side chat of that name is not forked: {forked}").into());
        }
        Ok(())
    }

    /// The owner sends the nested member one message by the one string, so
    /// that the member has a frame.
    async fn give_the_nested_member_a_frame(&self) -> TestResult {
        let (status, sent) =
            send_on_console(&self.app, &self.owner, &self.nested_by_one_string()).await;
        if !status.is_success() {
            return Err(format!("the owner's send to the nested member: {status} {sent}").into());
        }
        Ok(())
    }

    /// The answer of the console RPC `method` with `params` for the holder
    /// of `bearer`.
    async fn ask_as(&self, bearer: &str, method: &str, params: Value) -> Value {
        use tower::ServiceExt;
        let response = self
            .app
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

    /// [`Self::ask_as`] for the owner of the nested mob.
    async fn ask(&self, method: &str, params: Value) -> Value {
        self.ask_as(&self.owner, method, params).await
    }

    /// `answer` with every mob and every member a request of these rows
    /// can name replaced by one word each, so that two answers that differ
    /// only in the names their requests gave compare equal.
    fn apart_from_the_names(&self, answer: &Value) -> String {
        answer
            .to_string()
            .replace(&self.team, "<mob>")
            .replace(&self.nowhere(), "<mob>")
            .replace(NESTED_MEMBER, "<member>")
            .replace("nobody", "<member>")
    }

    /// The id of a mob the host does not have.
    fn nowhere(&self) -> String {
        format!("{}-nowhere", self.launch_mob)
    }
}

/// The frames of a timeline page, by their ids.
fn frame_ids(page: &Value) -> Vec<String> {
    page["result"]["frames"]
        .as_array()
        .map(|frames| {
            frames
                .iter()
                .filter_map(|frame| frame["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `answer` without what a member's status says of its progress, which
/// moves by itself between two equal requests.
fn apart_from_progress(answer: &Value) -> Value {
    let mut answer = answer.clone();
    if let Some(result) = answer.get_mut("result").and_then(Value::as_object_mut) {
        result.remove("progress");
    }
    answer
}

/// Whether `answer` is the refusal of a read of the roster of the door's
/// own mob for a `member_mob_id` that names another mob: invalid params,
/// in the sentence that says what `method` reads, with no data and no
/// result. The sentence names no mob and no member.
fn is_the_roster_refusal(answer: &Value, method: &str) -> bool {
    answer["error"]["code"] == json!(-32602)
        && answer["error"]["message"]
            == json!(format!(
                "{method} reads the roster of the mob the request is served for; \
                 member_mob_id names another mob"
            ))
        && answer["error"].get("data").is_none_or(Value::is_null)
        && answer["result"].is_null()
}

/// ONE MATCH. The two request forms reach one member. A member is named by
/// the one string its door knows it by, or by the mob it sits in
/// (`member_mob_id`) and its name there: a member of a mob nested under
/// the launch mob by that mob's id and its bare name, a member of the
/// launch mob by the launch mob's id and its name.
///
/// Each read that answers a record answers the nested member and the
/// launch mob's member, and answers the two forms with the same record.
/// The timeline read answers the nested member's frames by the one string
/// (the member was sent a message, so it has one) and the same frames by
/// the two values, and none of another member. The reads of the roster of
/// the door's own mob answer the launch mob's member by both forms, the
/// same apart from the progress a status reports, which moves by itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_by_mob_and_name_reaches_the_member_the_one_string_names() -> TestResult {
    let nested = Nested::start("member-mob-forms").await?;
    nested.give_the_nested_member_a_frame().await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    for (method, field) in READS_THAT_ANSWER_A_RECORD {
        for (who, by_string, by_mob) in [
            (
                "the member of the nested mob",
                json!({ *field: one_string }),
                json!({ *field: NESTED_MEMBER, "member_mob_id": nested.team }),
            ),
            (
                "the member of the launch mob",
                json!({ *field: LAUNCH_MEMBER }),
                json!({ *field: LAUNCH_MEMBER, "member_mob_id": nested.launch_mob }),
            ),
        ] {
            let named = nested.ask(method, by_string).await;
            let by_two = nested.ask(method, by_mob.clone()).await;
            if !named["error"].is_null() {
                wrong.push(format!(
                    "{method}: {who} is not answered by the one string: {named}"
                ));
            } else if by_two != named {
                wrong.push(format!(
                    "{method}: {who} named by {by_mob} is not answered what the one string is \
                     answered:\n  by two values: {by_two}\n  by the one string: {named}"
                ));
            }
        }
    }
    for (method, field) in ROSTER_READS {
        let named = nested.ask(method, json!({ *field: LAUNCH_MEMBER })).await;
        let by_two = nested
            .ask(
                method,
                json!({ *field: LAUNCH_MEMBER, "member_mob_id": nested.launch_mob }),
            )
            .await;
        if !named["error"].is_null() {
            wrong.push(format!(
                "{method}: the launch mob's member is not answered by its name: {named}"
            ));
        } else if apart_from_progress(&by_two) != apart_from_progress(&named) {
            wrong.push(format!(
                "{method}: the launch mob's member by two values: {by_two}\n  by its name: \
                 {named}"
            ));
        }
    }
    let by_string = nested
        .ask(
            "meta/console/query_timeline",
            json!({ "identity": one_string }),
        )
        .await;
    let by_two = nested
        .ask(
            "meta/console/query_timeline",
            json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team }),
        )
        .await;
    let of_another_member = |page: &Value| {
        page["result"]["frames"].as_array().is_some_and(|frames| {
            frames
                .iter()
                .any(|frame| frame["identity"] != json!(one_string))
        })
    };
    let first = frame_ids(&by_string);
    if !by_string["error"].is_null() || first.is_empty() || of_another_member(&by_string) {
        wrong.push(format!(
            "meta/console/query_timeline: the nested member's frames by the one string: \
             {by_string}"
        ));
    }
    let second = frame_ids(&by_two);
    if !by_two["error"].is_null()
        || of_another_member(&by_two)
        || !first.iter().all(|id| second.contains(id))
    {
        wrong.push(format!(
            "meta/console/query_timeline: the nested member's frames by two values: {by_two}\n  \
             by the one string: {by_string}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "the two request forms do not reach one member:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The reads of the roster of the door's own mob take the door's own mob
/// alone. `meta/get_member` and `meta/member_status` read the roster of
/// the mob the request is served for and reach no member of another mob.
/// A `member_mob_id` that names any other mob is refused as invalid
/// params, in one sentence that says so and names no mob: the same bytes
/// for a nested mob the caller is answered for, for one it is not
/// answered for, and for a mob the host does not have. By the one string
/// two members spell they answer the launch mob's member, which is the
/// one member of that name in their roster.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reads_of_the_doors_own_roster_take_no_other_mob() -> TestResult {
    let nested = Nested::start("member-mob-roster").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    for (method, field) in ROSTER_READS {
        let mut refusals = Vec::new();
        for (who, bearer, params) in [
            (
                "the owner, the nested mob and its member",
                &nested.owner,
                json!({ *field: NESTED_MEMBER, "member_mob_id": nested.team }),
            ),
            (
                "the owner, the nested mob and a name it does not have",
                &nested.owner,
                json!({ *field: "nobody", "member_mob_id": nested.team }),
            ),
            (
                "another person, the nested mob and its member",
                &nested.other,
                json!({ *field: NESTED_MEMBER, "member_mob_id": nested.team }),
            ),
            (
                "the owner, a mob the host does not have",
                &nested.owner,
                json!({ *field: NESTED_MEMBER, "member_mob_id": nested.nowhere() }),
            ),
        ] {
            let answer = nested.ask_as(bearer, method, params).await;
            if !is_the_roster_refusal(&answer, method) {
                wrong.push(format!("{method}, {who}: {answer}"));
            }
            refusals.push(answer);
        }
        if refusals.iter().any(|refusal| *refusal != refusals[0]) {
            wrong.push(format!(
                "{method}: the refusals are not the same bytes: {refusals:?}"
            ));
        }
        let shared = nested.ask(method, json!({ *field: one_string })).await;
        // A status read answers a name no member holds a result too, with
        // the status `unknown`: the member's own status is another.
        if !shared["error"].is_null() || shared["result"]["status"] == json!("unknown") {
            wrong.push(format!(
                "{method}: the one string two members spell does not answer the launch mob's \
                 member: {shared}"
            ));
        }
    }
    let member = nested
        .ask("meta/get_member", json!({ "member_id": one_string }))
        .await;
    if member["result"]["agent_identity"] != json!(one_string) {
        wrong.push(format!(
            "meta/get_member: the one string two members spell: {member}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "the reads of the door's own roster do not take the door's own mob alone:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// ZERO MATCHES. A `member_mob_id` that names no mob under the request's
/// mob, or a name the named mob does not have, is answered as a member the
/// request's mob does not have: each read that answers a record or a page
/// answers it what it answers a name no member holds, the names apart.
/// The reads of the roster of the door's own mob refuse the other mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_mob_id_that_reaches_no_member_is_answered_as_an_absent_member() -> TestResult {
    let nested = Nested::start("member-mob-absent").await?;
    let mut wrong = Vec::new();
    for (method, field) in READS_OF_ONE_MEMBER {
        let absent = nested.ask(method, json!({ *field: "nobody" })).await;
        for (what, params) in [
            (
                "a mob the door's mob has none of",
                json!({ *field: NESTED_MEMBER, "member_mob_id": nested.nowhere() }),
            ),
            (
                "a name the named mob does not have",
                json!({ *field: "nobody", "member_mob_id": nested.team }),
            ),
        ] {
            let answer = nested.ask(method, params).await;
            let as_it_should = if ROSTER_READS.iter().any(|(read, _)| read == method) {
                is_the_roster_refusal(&answer, method)
            } else {
                nested.apart_from_the_names(&answer) == nested.apart_from_the_names(&absent)
            };
            if !as_it_should {
                wrong.push(format!(
                    "{method}, {what}: {answer}\n  a name no member holds: {absent}"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a `member_mob_id` that reaches no member is not answered as an absent member:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// ZERO MATCHES, THE DOOR'S OWN MOB. The one string the door knows a live
/// nested member by, named together with the door's own mob, is a name of
/// the door's own mob, and that mob has no member of that name. Each read
/// answers what it answers a string of that form that no member holds, the
/// names apart; so do the send and the retire, and the nested member takes
/// no message from them and keeps its seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_doors_own_mob_and_the_one_string_of_a_nested_member_reach_no_member() -> TestResult {
    let nested = Nested::start("member-mob-own-absent").await?;
    nested.give_the_nested_member_a_frame().await?;
    let one_string = nested.nested_by_one_string();
    let no_string = format!("{}/nobody", nested.nowhere());
    let by_string = nested
        .ask(
            "meta/console/query_timeline",
            json!({ "identity": one_string }),
        )
        .await;
    if frame_ids(&by_string).is_empty() {
        return Err(format!(
            "the fixture: the nested member has no frame under the one string: {by_string}"
        )
        .into());
    }
    let named_with_the_own_mob =
        |field: &str| json!({ field: one_string, "member_mob_id": nested.launch_mob });
    let mut wrong = Vec::new();
    for (method, field) in READS_OF_ONE_MEMBER {
        let absent = nested.ask(method, json!({ *field: no_string })).await;
        let answer = nested.ask(method, named_with_the_own_mob(field)).await;
        if nested.apart_from_the_names(&answer) != nested.apart_from_the_names(&absent) {
            wrong.push(format!(
                "{method}: {answer}\n  a string no member holds: {absent}"
            ));
        }
    }
    let absent = nested
        .send_as(&nested.owner, json!({ "identity": no_string }), "absent")
        .await;
    let sent = nested
        .send_as(&nested.owner, named_with_the_own_mob("identity"), "own-mob")
        .await;
    if nested.apart_from_the_names(&sent) != nested.apart_from_the_names(&absent) {
        wrong.push(format!(
            "meta/console/send: {sent}\n  a string no member holds: {absent}"
        ));
    }
    if nested
        .page_holds(
            &nested.owner,
            json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team }),
            "own-mob",
        )
        .await
    {
        wrong.push("the nested member took the send that named the door's own mob".to_string());
    }
    let absent = nested
        .ask("meta/retire", json!({ "identity": no_string }))
        .await;
    let retired = nested
        .ask("meta/retire", named_with_the_own_mob("identity"))
        .await;
    if nested.apart_from_the_names(&retired) != nested.apart_from_the_names(&absent) {
        wrong.push(format!(
            "meta/retire: {retired}\n  a string no member holds: {absent}"
        ));
    }
    if !nested.nested_member_stays_seated().await?
        || !nested
            .owner_inspects(json!({ "identity": one_string }))
            .await
    {
        wrong.push(
            "the nested member lost its seat to a retire that named the door's own mob".to_string(),
        );
    }
    assert!(
        wrong.is_empty(),
        "the door's own mob and the one string of a nested member reach a member:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A caller who is not answered for a member learns nothing of it from the
/// two values. Another signed-in person names the owner's nested member by
/// the mob it sits in and its name there: each read method answers what it
/// answers that person for a mob the host does not have and a name no
/// member holds, the names apart. The member is still seated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_values_tell_a_caller_nothing_of_a_member_it_is_not_answered_for() -> TestResult {
    let nested = Nested::start("member-mob-hidden").await?;
    let mut wrong = Vec::new();
    for (method, field) in READS_OF_ONE_MEMBER {
        let hidden = nested
            .ask_as(
                &nested.other,
                method,
                json!({ *field: NESTED_MEMBER, "member_mob_id": nested.team }),
            )
            .await;
        let absent = nested
            .ask_as(
                &nested.other,
                method,
                json!({ *field: "nobody", "member_mob_id": nested.nowhere() }),
            )
            .await;
        if nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent) {
            wrong.push(format!(
                "{method}: the hidden member: {hidden}\n  no member: {absent}"
            ));
        }
    }
    let owned = nested
        .ask(
            "meta/console/inspect_identity",
            json!({ "identity": nested.nested_by_one_string() }),
        )
        .await;
    if !owned["error"].is_null() {
        wrong.push(format!("the owner no longer inspects the member: {owned}"));
    }
    assert!(
        wrong.is_empty(),
        "the two values tell a caller of a member it is not answered for:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// SEVERAL MATCHES. One string with two readings under one door: a member
/// of the launch mob whose name is `<the nested mob>/<a name>`, and the
/// member of that name in that mob. A caller who is answered for both and
/// names a member by that one string is refused by each read that answers
/// a record, a page or a list for both: invalid params, with the kind
/// `member_mob_id_required` and the string the request named, and with no
/// list of members.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_string_that_two_members_spell_is_refused_to_a_caller_answered_for_both() -> TestResult
{
    let nested = Nested::start("member-mob-shared").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    for (method, field) in READS_THAT_ANSWER_A_RECORD
        .iter()
        .chain(READS_THAT_ANSWER_A_LIST)
    {
        let answer = nested.ask(method, json!({ *field: one_string })).await;
        let error = &answer["error"];
        if error["code"] != json!(-32602)
            || error["data"]["kind"] != json!("member_mob_id_required")
            || error["data"]["identity"] != json!(one_string)
            || error["data"].get("candidates").is_some()
            || !answer["result"].is_null()
        {
            wrong.push(format!("{method}: {answer}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a request by one string that two members spell is not refused:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// THE SELECTION OF ONE OF TWO MEMBERS OF ONE SPELLING. Each of the two
/// members that spell one identity is reached by the mob it sits in and
/// its name there: the nested member by the nested mob's id and its bare
/// name, the launch mob's member by the launch mob's id and its whole
/// name. The inspect answers each as itself: its three values, and a
/// session of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_of_two_members_of_one_spelling_is_reached_by_its_two_values() -> TestResult {
    let nested = Nested::start("member-mob-selected").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    let mut sessions = Vec::new();
    for (who, member_mob, member_name) in [
        ("the nested member", nested.team.as_str(), NESTED_MEMBER),
        (
            "the launch mob's member",
            nested.launch_mob.as_str(),
            one_string.as_str(),
        ),
    ] {
        let answer = nested
            .ask(
                "meta/console/inspect_identity",
                json!({ "identity": member_name, "member_mob_id": member_mob }),
            )
            .await;
        let record = &answer["result"]["identity"];
        if record["mob_id"] != json!(nested.launch_mob)
            || record["member_mob_id"] != json!(member_mob)
            || record["member_name"] != json!(member_name)
            || record["identity"] != json!(one_string)
        {
            wrong.push(format!("{who}: {answer}"));
        }
        sessions.push(record["session_id"].clone());
    }
    if sessions[0] == sessions[1] || sessions.iter().any(Value::is_null) {
        wrong.push(format!(
            "the two members are not answered a session each: {sessions:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "each of two members of one spelling is not reached by its two values:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A caller who is answered for one of two members of one spelling is
/// answered for that one by the one string, exactly as if the other did
/// not exist. Another signed-in person asks each read method that answers
/// a record for the launch mob's member whose name is the one string,
/// before the owner's nested mob exists and after: the two answers are
/// the same bytes, apart from the progress a status reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_answered_for_one_of_two_members_is_answered_as_if_the_other_did_not_exist()
-> TestResult {
    let nested = Nested::start_without_the_nested_mob("member-mob-one-of-two").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let mut before = Vec::new();
    for (method, field) in STEADY_READS_OF_A_LAUNCH_MEMBER {
        before.push(
            nested
                .ask_as(&nested.other, method, json!({ *field: one_string }))
                .await,
        );
    }
    nested.create_the_nested_mob().await?;
    let mut wrong = Vec::new();
    for ((method, field), before) in STEADY_READS_OF_A_LAUNCH_MEMBER.iter().zip(before) {
        let after = nested
            .ask_as(&nested.other, method, json!({ *field: one_string }))
            .await;
        if !before["error"].is_null() {
            wrong.push(format!(
                "{method}: the launch mob's member is not answered before the nested mob \
                 exists: {before}"
            ));
        } else if apart_from_progress(&after) != apart_from_progress(&before) {
            wrong.push(format!(
                "{method}: after the nested mob exists: {after}\n  before: {before}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a caller answered for one of two members is not answered as if the other did not \
         exist:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// `member_mob_id` on a method that does not take it is refused, never
/// ignored: invalid params, in the sentence that names the method, with
/// the kind `member_mob_id_not_accepted` and the method. A method that
/// lists members, a method that reads a mob, a method of the host and a
/// method that names several members take none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_mob_id_on_a_method_that_does_not_take_it_is_refused() -> TestResult {
    let nested = Nested::start("member-mob-refused").await?;
    let mut wrong = Vec::new();
    for (method, params) in [
        ("meta/list_members", json!({})),
        ("meta/status", json!({})),
        ("meta/access/status", json!({})),
        (
            "meta/fork_member",
            json!({ "source_member_id": LAUNCH_MEMBER, "agent_identity": "alpha-second-side" }),
        ),
    ] {
        let mut params = params;
        params["member_mob_id"] = json!(nested.team);
        let answer = nested.ask(method, params).await;
        if !is_the_not_accepted_refusal(&answer, method) {
            wrong.push(format!("{method}: {answer}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "`member_mob_id` is not refused on a method that does not take it:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// An inspect names the member it resolved by the three values of its row:
/// the mob whose door answered (`mob_id`), the mob the member sits in
/// (`member_mob_id`) and its name there (`member_name`), whichever form
/// the request named it by.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inspect_names_the_member_it_resolved_by_its_three_values() -> TestResult {
    let nested = Nested::start("member-mob-inspect").await?;
    let mut wrong = Vec::new();
    for (who, params, member_mob, member_name) in [
        (
            "the member of the nested mob, by the one string",
            json!({ "identity": nested.nested_by_one_string() }),
            nested.team.as_str(),
            NESTED_MEMBER,
        ),
        (
            "the member of the nested mob, by two values",
            json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team }),
            nested.team.as_str(),
            NESTED_MEMBER,
        ),
        (
            "the member of the launch mob",
            json!({ "identity": LAUNCH_MEMBER }),
            nested.launch_mob.as_str(),
            LAUNCH_MEMBER,
        ),
    ] {
        let answer = nested.ask("meta/console/inspect_identity", params).await;
        let record = &answer["result"]["identity"];
        if record["mob_id"] != json!(nested.launch_mob)
            || record["member_mob_id"] != json!(member_mob)
            || record["member_name"] != json!(member_name)
        {
            wrong.push(format!("{who}: {answer}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "an inspect does not name the member it resolved by its three values:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The listing of identities names each member by the three values of its
/// row, as an inspect names the member it resolved: the mob whose door
/// answered (`mob_id`), the mob the member sits in (`member_mob_id`) and
/// its name there (`member_name`). The console RPC and the route answer
/// them alike, so a client tells two members of one string apart by a
/// named field.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_listing_of_identities_names_each_member_by_its_three_values() -> TestResult {
    use tower::ServiceExt;
    let nested = Nested::start("member-mob-listing").await?;
    let one_string = nested.nested_by_one_string();
    // Each member by the identity its entry carries, with the mob it sits
    // in and its name there.
    let members = [
        (LAUNCH_MEMBER, nested.launch_mob.as_str(), LAUNCH_MEMBER),
        (one_string.as_str(), nested.team.as_str(), NESTED_MEMBER),
    ];
    // What a listing says of the two members: each member's identity with
    // the three values its entry carries.
    let named = |listing: Option<&Value>| -> Vec<String> {
        members
            .iter()
            .map(|(identity, _, _)| {
                let entry = listing
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find(|entry| entry["identity"] == json!(identity));
                match entry {
                    Some(entry) => format!(
                        "{identity}: {} {} {}",
                        entry["mob_id"], entry["member_mob_id"], entry["member_name"]
                    ),
                    None => format!("{identity}: not listed"),
                }
            })
            .collect()
    };
    let wanted: Vec<String> = members
        .iter()
        .map(|(identity, mob, name)| {
            format!(
                "{identity}: {} {} {}",
                json!(nested.launch_mob),
                json!(mob),
                json!(name)
            )
        })
        .collect();
    // A member is listed a moment after it is seated: the listings are
    // asked until they answer as expected, or ten seconds passed.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut by_rpc: Vec<String>;
    let mut by_route: Vec<String>;
    loop {
        let rpc = nested.ask("meta/console/list_identities", json!({})).await;
        by_rpc = named(rpc.pointer("/result/identities"));
        let response = nested
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/console/identities")
                    .header(header::AUTHORIZATION, format!("Bearer {}", nested.owner))
                    .body(axum::body::Body::empty())
                    .expect("identities request"),
            )
            .await
            .expect("identities response");
        let body = axum::body::to_bytes(response.into_body(), 1 << 22)
            .await
            .expect("identities body");
        let route: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        by_route = named(route.pointer("/identities"));
        if (by_rpc == wanted && by_route == wanted) || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        by_rpc, wanted,
        "the console RPC's listing of identities names each member by its three values"
    );
    assert_eq!(
        by_route, wanted,
        "the route's listing of identities names each member by its three values"
    );
    Ok(())
}

/// THE MIRROR. A caller who is answered for the nested member alone is
/// answered that member by the one string. Another person holds a member
/// of the launch mob under the one string the door knows the owner's
/// nested member by (a side chat of its own, which the owner is not
/// answered for). The owner names its nested member by the one string:
/// each read that answers a record answers the nested member, with the
/// record the two values are answered. The other person is answered its
/// own member by the one string.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_answered_for_the_nested_member_alone_is_answered_it_by_the_one_string()
-> TestResult {
    // The other person's member exists first: a name that a member hidden
    // from a person holds is refused to that person as taken.
    let nested = Nested::start_without_the_nested_mob("member-mob-mirror").await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    for (method, field) in READS_THAT_ANSWER_A_RECORD {
        let named = nested.ask(method, json!({ *field: one_string })).await;
        let by_two = nested
            .ask(
                method,
                json!({ *field: NESTED_MEMBER, "member_mob_id": nested.team }),
            )
            .await;
        if !named["error"].is_null() {
            wrong.push(format!(
                "{method}: the owner is not answered its nested member by the one string: \
                 {named}"
            ));
        } else if named != by_two {
            wrong.push(format!(
                "{method}: by the one string: {named}\n  by two values: {by_two}"
            ));
        }
    }
    let owned = nested
        .ask(
            "meta/console/inspect_identity",
            json!({ "identity": one_string }),
        )
        .await;
    let record = &owned["result"]["identity"];
    if record["mob_id"] != json!(nested.launch_mob)
        || record["member_mob_id"] != json!(nested.team)
        || record["member_name"] != json!(NESTED_MEMBER)
    {
        wrong.push(format!(
            "the record the owner is answered by the one string does not name its nested \
             member by its three values: {owned}"
        ));
    }
    for (method, field) in READS_THAT_ANSWER_A_RECORD {
        let others = nested
            .ask_as(&nested.other, method, json!({ *field: one_string }))
            .await;
        if !others["error"].is_null() {
            wrong.push(format!(
                "{method}: the other person is not answered its own member by the one \
                 string: {others}"
            ));
        }
    }
    let others = nested
        .ask_as(
            &nested.other,
            "meta/console/inspect_identity",
            json!({ "identity": one_string }),
        )
        .await;
    if others["result"]["identity"]["labels"]["side_chat_of"] != json!(LAUNCH_MEMBER) {
        wrong.push(format!(
            "the other person is not answered its own member by the one string: {others}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a caller answered for the nested member alone is not answered it by the one \
         string:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A caller who is answered for neither of two members of one spelling is
/// answered as for no member. The owner holds both: its nested member, and
/// a side chat of its own in the launch mob under the one string the door
/// knows that nested member by. Another person names a member by that one
/// string: each read answers what it answers that person for a mob the
/// host does not have and a name no member holds, the names apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_answered_for_neither_of_two_members_is_answered_as_for_no_member() -> TestResult {
    let nested = Nested::start("member-mob-neither").await?;
    nested.fork_the_namesake_as(&nested.owner).await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    for (method, field) in READS_OF_ONE_MEMBER {
        let hidden = nested
            .ask_as(&nested.other, method, json!({ *field: one_string }))
            .await;
        let absent = nested
            .ask_as(
                &nested.other,
                method,
                json!({ *field: format!("{}/nobody", nested.nowhere()) }),
            )
            .await;
        if nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent) {
            wrong.push(format!(
                "{method}: the two hidden members: {hidden}\n  no member: {absent}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a caller answered for neither of two members is not answered as for no member:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The content of the message a row sends under `key`: no two keys of one
/// row give contents of which one holds the other.
fn content_of(key: &str) -> String {
    format!("a word under the key {key}.")
}

/// Whether `answer` is the refusal of `member_mob_id` by a method that does
/// not take it: invalid params, in the sentence that names the method, with
/// the kind and the method as its data, and no result. The refusals of a
/// member's naming are two kinds, this one and `member_mob_id_required`: a
/// third would be a breaking change.
fn is_the_not_accepted_refusal(answer: &Value, method: &str) -> bool {
    let error = &answer["error"];
    error["code"] == json!(-32602)
        && error["message"] == json!(format!("{method} does not take member_mob_id"))
        && error["data"] == json!({ "kind": "member_mob_id_not_accepted", "method": method })
        && answer["result"].is_null()
}

/// Whether `answer` is the refusal of one string that two members spell:
/// invalid params, the kind `member_mob_id_required`, the string the
/// request named, no list of members and no result.
fn is_the_shared_spelling_refusal(answer: &Value, one_string: &str) -> bool {
    let error = &answer["error"];
    error["code"] == json!(-32602)
        && error["data"]["kind"] == json!("member_mob_id_required")
        && error["data"]["identity"] == json!(one_string)
        && error["data"].get("candidates").is_none()
        && answer["result"].is_null()
}

impl Nested {
    /// The holder of `bearer` sends the member the one string names one
    /// message through the send route, under `key` and with that key's
    /// content. The accepted send's answer.
    async fn send_by_the_route_as(&self, bearer: &str, key: &str) -> Result<Value, String> {
        use tower::ServiceExt;
        let response = self
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/console/send")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(axum::body::Body::from(
                        json!({
                            "identity": self.nested_by_one_string(),
                            "content": content_of(key),
                            "origin": "console",
                            "idempotency_key": key,
                        })
                        .to_string(),
                    ))
                    .expect("send request"),
            )
            .await
            .expect("send response");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("send body");
        let sent: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if status.is_success() {
            Ok(sent)
        } else {
            Err(format!("{status} {sent}"))
        }
    }

    /// The answer of a send on the request door for the holder of `bearer`:
    /// `member` names the member (its fields are the request's), `key` is
    /// the send's client key and gives its content.
    async fn send_as(&self, bearer: &str, member: Value, key: &str) -> Value {
        let mut params = member;
        params["content"] = json!(content_of(key));
        params["origin"] = json!("console");
        params["idempotency_key"] = json!(key);
        self.ask_as(bearer, "meta/console/send", params).await
    }

    /// The session the inspect answers the holder of `bearer` for the
    /// member `member` names; null where it answers no member.
    async fn session_of(&self, bearer: &str, member: Value) -> Value {
        self.ask_as(bearer, "meta/console/inspect_identity", member)
            .await["result"]["identity"]["session_id"]
            .clone()
    }

    /// Whether the timeline page the holder of `bearer` reads for the
    /// member `member` names holds the message sent under `key`.
    async fn page_holds(&self, bearer: &str, member: Value, key: &str) -> bool {
        self.ask_as(bearer, "meta/console/query_timeline", member)
            .await
            .to_string()
            .contains(&content_of(key))
    }

    /// Whether the owner's inspect answers the member named by `params`.
    async fn owner_inspects(&self, params: Value) -> bool {
        self.ask("meta/console/inspect_identity", params).await["error"].is_null()
    }

    /// The state the status read answers the owner for the member `member`
    /// names; null where it answers no member.
    async fn state_of(&self, member: Value) -> Value {
        self.ask("meta/status_identity", member).await["result"]["state"].clone()
    }

    /// Whether the nested mob's roster holds its member.
    async fn nested_member_is_seated(&self) -> TestResult<bool> {
        let mobs = self
            .runtime
            .agent_mob_mcp_state()
            .ok_or("the runtime carries agent mob tools")?;
        let handle = Box::pin(mobs.handle_for(&meerkat_mob::MobId::from(self.team.as_str())))
            .await
            .map_err(|error| error.to_string())?;
        Ok(handle
            .get_member(&AgentIdentity::from(NESTED_MEMBER))
            .await?
            .is_some())
    }

    /// Whether the nested member is retired, within ten seconds: it has
    /// left its mob's roster, and the status read answers the owner for
    /// its two values what it answers for a mob the host does not have
    /// and a name no member holds, the names apart.
    async fn nested_member_is_retired(&self) -> bool {
        let the_nested = json!({ "identity": NESTED_MEMBER, "member_mob_id": self.team });
        let nobody = json!({ "identity": "nobody", "member_mob_id": self.nowhere() });
        for _ in 0..100 {
            let left = matches!(self.nested_member_is_seated().await, Ok(false));
            if left
                && self.apart_from_the_names(
                    &self.ask("meta/status_identity", the_nested.clone()).await,
                ) == self
                    .apart_from_the_names(&self.ask("meta/status_identity", nobody.clone()).await)
            {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        false
    }

    /// [`Self::ask`], given up after ten seconds.
    async fn ask_within(&self, method: &str, params: Value) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(10), self.ask(method, params))
            .await
            .unwrap_or_else(|_| json!({ "no answer within ten seconds": method }))
    }
}

/// The timeline by one string that two members spell answers each caller
/// the frames of the member it is answered for, and no frame of the other.
/// Another person holds a member of the launch mob under the one string the
/// door knows the owner's nested member by. Each of the two persons sends
/// its own member a message of its own by that string and reads the
/// timeline by it: the page holds the message that person sent and not the
/// message the other person sent. The sends go through the send route.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_timeline_by_one_string_two_members_spell_answers_each_caller_its_own_members_frames()
-> TestResult {
    let wrong = each_callers_page_holds_its_own_message("member-mob-frames", true).await?;
    assert!(
        wrong.is_empty(),
        "the timeline by one string that two members spell does not answer each caller its \
         own member's frames:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// [`the_timeline_by_one_string_two_members_spell_answers_each_caller_its_own_members_frames`]
/// with the two sends on the request door: the row of the timeline alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_timeline_by_one_string_answers_each_caller_its_own_after_sends_on_the_request_door()
-> TestResult {
    let wrong = each_callers_page_holds_its_own_message("member-mob-frames-door", false).await?;
    assert!(
        wrong.is_empty(),
        "the timeline by one string that two members spell does not answer each caller its \
         own member's frames:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// What the two timeline rows of one string that two members spell hold,
/// over a launch mob named `tag`: one line for each way a caller's page is
/// not its own member's. The two sends go through the send route
/// (`by_the_route`) or on the request door.
async fn each_callers_page_holds_its_own_message(
    tag: &str,
    by_the_route: bool,
) -> TestResult<Vec<String>> {
    let nested = Nested::start_without_the_nested_mob(tag).await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let one_string = nested.nested_by_one_string();
    let persons = [
        (
            "the other person",
            &nested.other,
            "of-the-other",
            "of-the-owner",
        ),
        ("the owner", &nested.owner, "of-the-owner", "of-the-other"),
    ];
    let mut wrong = Vec::new();
    for (who, bearer, own, _) in persons {
        let refused = if by_the_route {
            nested.send_by_the_route_as(bearer, own).await.err()
        } else {
            let sent = nested
                .send_as(bearer, json!({ "identity": one_string }), own)
                .await;
            (!sent["error"].is_null() || sent["result"].is_null()).then(|| sent.to_string())
        };
        if let Some(refused) = refused {
            wrong.push(format!(
                "{who}'s send by the one string is not taken by its own member: {refused}"
            ));
        }
    }
    for (who, bearer, own, others) in persons {
        let by_string = json!({ "identity": one_string });
        if !nested.page_holds(bearer, by_string.clone(), own).await {
            wrong.push(format!(
                "{who}'s page does not hold the message it sent its own member"
            ));
        }
        if nested.page_holds(bearer, by_string, others).await {
            wrong.push(format!(
                "{who}'s page holds the message sent to the member it is not answered for"
            ));
        }
    }
    Ok(wrong)
}

/// A SEND, NO MATCH AND ONE MATCH. A send that names its member by the mob
/// it sits in and its name there is taken by that member: its acceptance
/// names the session the inspect answers for the member by its two values,
/// as the acceptance of a send by the one string does. A send whose two
/// values reach no member is answered what a send to a name no member
/// holds is answered, the names apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_by_mob_and_name_is_taken_by_the_member_it_names() -> TestResult {
    let nested = Nested::start("member-mob-send").await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    let absent = nested
        .send_as(&nested.owner, json!({ "identity": "nobody" }), "absent")
        .await;
    for (what, member, key) in [
        (
            "a mob the door's mob has none of",
            json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.nowhere() }),
            "no-mob",
        ),
        (
            "a name the named mob does not have",
            json!({ "identity": "nobody", "member_mob_id": nested.team }),
            "no-name",
        ),
    ] {
        let answer = nested.send_as(&nested.owner, member, key).await;
        if nested.apart_from_the_names(&answer) != nested.apart_from_the_names(&absent) {
            wrong.push(format!(
                "{what}: {answer}\n  a name no member holds: {absent}"
            ));
        }
    }
    let by_two_values = json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team });
    let session = nested
        .session_of(&nested.owner, by_two_values.clone())
        .await;
    if session.is_null()
        || session
            != nested
                .session_of(&nested.owner, json!({ "identity": one_string }))
                .await
    {
        wrong.push(format!(
            "the inspect does not answer the nested member one session by its two forms; by \
             two values: {session}"
        ));
    }
    for (form, member, key) in [
        (
            "the one string",
            json!({ "identity": one_string }),
            "by-string",
        ),
        ("two values", by_two_values, "by-two"),
    ] {
        let sent = nested.send_as(&nested.owner, member, key).await;
        if !sent["error"].is_null() || sent["result"]["session_id"] != session {
            wrong.push(format!(
                "a send to the nested member by {form} is not taken on its session {session}: \
                 {sent}"
            ));
        }
    }
    let own_member = json!({ "identity": LAUNCH_MEMBER, "member_mob_id": nested.launch_mob });
    let own_session = nested.session_of(&nested.owner, own_member.clone()).await;
    let own = nested.send_as(&nested.owner, own_member, "own").await;
    if own_session.is_null() || own["result"]["session_id"] != own_session {
        wrong.push(format!(
            "a send to the launch mob's member by two values is not taken on its session \
             {own_session}: {own}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a send by mob and name is not taken by the member it names:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A SEND, SEVERAL MATCHES AND THE SELECTION. A send by one string that
/// two members the caller is answered for spell is refused and neither
/// member takes it: its message is on neither member's page. A send by the
/// two values of each is taken on that member's session, the session the
/// inspect answers for it, and its message is on that member's page alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_by_one_string_two_members_spell_is_refused_and_each_takes_its_own() -> TestResult {
    let nested = Nested::start("member-mob-send-shared").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let the_nested = json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team });
    let the_launch = json!({ "identity": one_string, "member_mob_id": nested.launch_mob });
    let nested_session = nested.session_of(&nested.owner, the_nested.clone()).await;
    let launch_session = nested.session_of(&nested.owner, the_launch.clone()).await;
    let members = [
        (
            "the nested member",
            &the_nested,
            &nested_session,
            "to-the-nested",
            "to-the-launch",
        ),
        (
            "the launch mob's member",
            &the_launch,
            &launch_session,
            "to-the-launch",
            "to-the-nested",
        ),
    ];
    let mut wrong = Vec::new();
    if nested_session.is_null() || launch_session.is_null() || nested_session == launch_session {
        wrong.push(format!(
            "the inspect does not answer the two members a session each by their two values: \
             {nested_session} / {launch_session}"
        ));
    }
    let refused = nested
        .send_as(&nested.owner, json!({ "identity": one_string }), "shared")
        .await;
    if !is_the_shared_spelling_refusal(&refused, &one_string) {
        wrong.push(format!("the send by the one string: {refused}"));
    }
    for (who, member, _, _, _) in members {
        if nested
            .page_holds(&nested.owner, member.clone(), "shared")
            .await
        {
            wrong.push(format!("{who} took the send by the one string"));
        }
    }
    for (who, member, session, own, _) in members {
        let sent = nested.send_as(&nested.owner, member.clone(), own).await;
        if !sent["error"].is_null() || sent["result"]["session_id"] != *session {
            wrong.push(format!(
                "the send to {who} is not taken on its session {session}: {sent}"
            ));
        }
    }
    for (who, member, _, own, others) in members {
        if !nested.page_holds(&nested.owner, member.clone(), own).await {
            wrong.push(format!(
                "the page of {who} does not hold the message sent to it"
            ));
        }
        if nested
            .page_holds(&nested.owner, member.clone(), others)
            .await
        {
            wrong.push(format!(
                "the page of {who} holds the message sent to the other member"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a send under a shared spelling is not refused by the one string and taken by each \
         member's two values:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A SEND, THE CALLERS WHO ARE NOT ANSWERED FOR BOTH. A person who is not
/// answered for the nested member sends by its two values and is answered
/// what a send to a mob and a name the host does not have is answered,
/// the names apart. And the mirror: while another person holds a member of
/// the launch mob under the one string, the owner's send by that string is
/// taken on the session of its nested member, and its message is not on
/// the other person's page.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_tells_nothing_of_a_hidden_member_and_reaches_the_one_the_caller_has() -> TestResult
{
    let nested = Nested::start_without_the_nested_mob("member-mob-send-mirror").await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let one_string = nested.nested_by_one_string();
    let the_nested = json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team });
    let mut wrong = Vec::new();
    let hidden = nested
        .send_as(&nested.other, the_nested.clone(), "hidden")
        .await;
    let absent = nested
        .send_as(
            &nested.other,
            json!({ "identity": "nobody", "member_mob_id": nested.nowhere() }),
            "absent",
        )
        .await;
    if nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent) {
        wrong.push(format!(
            "the other person's send to the hidden member: {hidden}\n  to no member: {absent}"
        ));
    }
    let session = nested.session_of(&nested.owner, the_nested).await;
    let mirrored = nested
        .send_as(&nested.owner, json!({ "identity": one_string }), "mirror")
        .await;
    if !mirrored["error"].is_null()
        || session.is_null()
        || mirrored["result"]["session_id"] != session
    {
        wrong.push(format!(
            "the owner's send by the one string is not taken on the session {session} of its \
             nested member: {mirrored}"
        ));
    }
    if nested
        .page_holds(&nested.other, json!({ "identity": one_string }), "mirror")
        .await
    {
        wrong.push("the other person's page holds the owner's message".to_string());
    }
    assert!(
        wrong.is_empty(),
        "a send tells of a hidden member, or does not reach the one the caller has:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A SEND BY A CALLER ANSWERED FOR NEITHER of two members of one spelling
/// is answered as a send to no member, and is never told to name a mob.
/// The owner holds both: its nested member, and a side chat of its own in
/// the launch mob under the one string. Another person sends by that
/// string: it is answered what a send to a mob and a name the host does
/// not have is answered, the names apart, and neither member takes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_by_a_caller_answered_for_neither_of_two_members_is_answered_as_for_no_member()
-> TestResult {
    let nested = Nested::start("member-mob-send-neither").await?;
    nested.fork_the_namesake_as(&nested.owner).await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    let hidden = nested
        .send_as(&nested.other, json!({ "identity": one_string }), "hidden")
        .await;
    let absent = nested
        .send_as(
            &nested.other,
            json!({ "identity": format!("{}/nobody", nested.nowhere()) }),
            "absent",
        )
        .await;
    if hidden["error"]["data"]["kind"] == json!("member_mob_id_required")
        || nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent)
    {
        wrong.push(format!(
            "the send to the two hidden members: {hidden}\n  to no member: {absent}"
        ));
    }
    for (who, member) in [
        (
            "the nested member",
            json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team }),
        ),
        (
            "the launch mob's member",
            json!({ "identity": one_string, "member_mob_id": nested.launch_mob }),
        ),
    ] {
        if nested.page_holds(&nested.owner, member, "hidden").await {
            wrong.push(format!("{who} took the other person's send"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a send by a caller answered for neither of two members is not answered as for no \
         member:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A RETIRE, NO MATCH AND ONE MATCH. A retire whose two values reach no
/// member is answered what a retire of a name no member holds is answered,
/// the names apart, and retires nobody. A retire that names the nested
/// member by the mob it sits in and its name there retires that member
/// (it leaves its mob's roster, and the status read by its two values
/// answers it as no member), and the launch mob's member stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retire_by_mob_and_name_retires_the_member_it_names() -> TestResult {
    let nested = Nested::start("member-mob-retire").await?;
    let one_string = nested.nested_by_one_string();
    let the_nested = json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team });
    let mut wrong = Vec::new();
    let absent = nested
        .ask("meta/retire", json!({ "identity": "nobody" }))
        .await;
    for (what, params) in [
        (
            "a mob the door's mob has none of",
            json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.nowhere() }),
        ),
        (
            "a name the named mob does not have",
            json!({ "identity": "nobody", "member_mob_id": nested.team }),
        ),
    ] {
        let answer = nested.ask("meta/retire", params).await;
        if nested.apart_from_the_names(&answer) != nested.apart_from_the_names(&absent) {
            wrong.push(format!(
                "{what}: {answer}\n  a name no member holds: {absent}"
            ));
        }
    }
    if !nested
        .owner_inspects(json!({ "identity": one_string }))
        .await
    {
        wrong.push("a retire that reached no member retired the nested member".to_string());
    }
    let seated = nested.state_of(the_nested.clone()).await;
    let retired = nested.ask("meta/retire", the_nested).await;
    if seated.is_null() || !retired["error"].is_null() {
        wrong.push(format!(
            "the retire of the nested member by two values (its state before: {seated}): \
             {retired}"
        ));
    } else if !nested.nested_member_is_retired().await {
        wrong.push(
            "the nested member has not left its roster, or is still answered by its two \
             values, after its retire"
                .to_string(),
        );
    }
    if !nested
        .owner_inspects(json!({ "identity": LAUNCH_MEMBER }))
        .await
    {
        wrong.push("the launch mob's member does not stay".to_string());
    }
    assert!(
        wrong.is_empty(),
        "a retire by mob and name does not retire the member it names:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A RETIRE, SEVERAL MATCHES AND THE SELECTION. A retire by one string
/// that two members the caller is answered for spell is refused and
/// retires neither; a retire by the two values of the nested member
/// retires that one (it leaves its mob's roster, and the status read by its
/// two values answers it as no member), and the launch mob's member of
/// that name stays and is then named by the one string again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retire_by_one_string_two_members_spell_is_refused_and_each_is_retired_by_its_own()
-> TestResult {
    let nested = Nested::start("member-mob-retire-shared").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let the_nested = json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team });
    let the_launch = json!({ "identity": one_string, "member_mob_id": nested.launch_mob });
    let mut wrong = Vec::new();
    let seated = nested.state_of(the_nested.clone()).await;
    let refused = nested
        .ask("meta/retire", json!({ "identity": one_string }))
        .await;
    if !is_the_shared_spelling_refusal(&refused, &one_string) {
        wrong.push(format!("the retire by the one string: {refused}"));
    }
    for (who, member) in [
        ("the nested member", &the_nested),
        ("the launch mob's member", &the_launch),
    ] {
        if !nested.owner_inspects(member.clone()).await {
            wrong.push(format!("{who} does not stay after the refused retire"));
        }
    }
    let retired = nested.ask("meta/retire", the_nested).await;
    if seated.is_null() || !retired["error"].is_null() {
        wrong.push(format!(
            "the retire of the nested member by two values (its state before: {seated}): \
             {retired}"
        ));
    } else if !nested.nested_member_is_retired().await {
        wrong.push(
            "the nested member has not left its roster, or is still answered by its two \
             values, after its retire"
                .to_string(),
        );
    }
    let left = nested
        .ask(
            "meta/console/inspect_identity",
            json!({ "identity": one_string }),
        )
        .await;
    if left["result"]["identity"]["member_mob_id"] != json!(nested.launch_mob) {
        wrong.push(format!(
            "the launch mob's member is not the one the string names once the nested member \
             is retired: {left}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a retire under a shared spelling is not refused by the one string and made by each \
         member's two values:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A RETIRE, THE CALLERS WHO ARE NOT ANSWERED FOR BOTH. A person who is
/// not answered for the nested member retires it by its two values and is
/// answered what a retire of a mob and a name the host does not have is
/// answered, the names apart. And the mirror: while another person holds a
/// member of the launch mob under the one string, the owner's retire by
/// that string retires its nested member (it leaves its mob's roster, and
/// the status read by its two values answers it as no member), and the
/// other person's member stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retire_tells_nothing_of_a_hidden_member_and_reaches_the_one_the_caller_has() -> TestResult
{
    let nested = Nested::start_without_the_nested_mob("member-mob-retire-mirror").await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let one_string = nested.nested_by_one_string();
    let the_nested = json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team });
    let mut wrong = Vec::new();
    let hidden = nested
        .ask_as(&nested.other, "meta/retire", the_nested.clone())
        .await;
    let absent = nested
        .ask_as(
            &nested.other,
            "meta/retire",
            json!({ "identity": "nobody", "member_mob_id": nested.nowhere() }),
        )
        .await;
    if nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent) {
        wrong.push(format!(
            "the other person's retire of the hidden member: {hidden}\n  of no member: {absent}"
        ));
    }
    let seated = nested.state_of(the_nested.clone()).await;
    let mirrored = nested
        .ask("meta/retire", json!({ "identity": one_string }))
        .await;
    if seated.is_null() || !mirrored["error"].is_null() {
        wrong.push(format!(
            "the owner's retire by the one string does not retire its nested member (its \
             state before: {seated}): {mirrored}"
        ));
    } else if !nested.nested_member_is_retired().await {
        wrong.push(
            "the nested member has not left its roster, or is still answered by its two \
             values, after the owner's retire"
                .to_string(),
        );
    }
    let others = nested
        .ask_as(
            &nested.other,
            "meta/console/inspect_identity",
            json!({ "identity": one_string }),
        )
        .await;
    if others["result"]["identity"]["labels"]["side_chat_of"] != json!(LAUNCH_MEMBER) {
        wrong.push(format!(
            "the other person's member does not stay after the owner's retire: {others}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a retire tells of a hidden member, or does not reach the one the caller has:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A RETIRE BY A CALLER ANSWERED FOR NEITHER of two members of one
/// spelling is answered as a retire of no member, and is never told to
/// name a mob. The owner holds both: its nested member, and a side chat of
/// its own in the launch mob under the one string. Another person retires
/// by that string: it is answered what a retire of a mob and a name the
/// host does not have is answered, the names apart, and the owner's side
/// chat and its nested member stay in their rosters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retire_by_a_caller_answered_for_neither_of_two_members_is_answered_as_for_no_member()
-> TestResult {
    let nested = Nested::start("member-mob-retire-neither").await?;
    nested.fork_the_namesake_as(&nested.owner).await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    let hidden = nested
        .ask_as(
            &nested.other,
            "meta/retire",
            json!({ "identity": one_string }),
        )
        .await;
    let absent = nested
        .ask_as(
            &nested.other,
            "meta/retire",
            json!({ "identity": format!("{}/nobody", nested.nowhere()) }),
        )
        .await;
    if hidden["error"]["data"]["kind"] == json!("member_mob_id_required")
        || nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent)
    {
        wrong.push(format!(
            "the retire of the two hidden members: {hidden}\n  of no member: {absent}"
        ));
    }
    let roster = nested.ask("meta/list_members", json!({})).await;
    let stays = roster["result"].as_array().is_some_and(|members| {
        members
            .iter()
            .any(|member| member["agent_identity"] == json!(one_string))
    });
    if !stays {
        wrong.push(format!(
            "the owner's side chat does not stay in the roster: {roster}"
        ));
    }
    if !nested.nested_member_is_seated().await? {
        wrong.push("the owner's nested member does not stay in its roster".to_string());
    }
    assert!(
        wrong.is_empty(),
        "a retire by a caller answered for neither of two members is not answered as for no \
         member:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// What a method of one member does with `member_mob_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Takes {
    /// It reads or acts on the member the two values name.
    TheTwoValues,
    /// It reads the roster of the door's own mob and takes that mob alone.
    TheOwnMobAlone,
    /// It does not take the field and refuses a request that carries it.
    Nothing,
}

/// The methods of the door that read or act on one member they name, each
/// with what it does with `member_mob_id`. The schedule methods, the
/// memory methods, the tool grant methods and every other method without a
/// row of its own in this file refuse the field, and read one string that
/// two members spell as the launch mob's member, as before
/// (`a_method_that_refuses_the_field_reads_the_one_string_as_the_launch_mobs_member`).
/// A memory method reaches a nested mob's member by neither form: it
/// refuses the field, and it takes no identity that holds `/`
/// (`a_memory_method_takes_no_string_of_a_mob_and_a_name`).
/// The retire is last: it ends the member it reaches.
const METHODS_OF_ONE_MEMBER: &[(&str, Takes)] = &[
    ("meta/get_member", Takes::TheOwnMobAlone),
    ("meta/member_status", Takes::TheOwnMobAlone),
    ("meta/member_health", Takes::Nothing),
    ("meta/status_identity", Takes::TheTwoValues),
    ("meta/inspect_identity", Takes::TheTwoValues),
    ("meta/console/inspect_identity", Takes::TheTwoValues),
    ("meta/identity/resolved_tools", Takes::Nothing),
    ("meta/identity/routing_status", Takes::Nothing),
    ("meta/identity/warm", Takes::Nothing),
    ("meta/send_status", Takes::Nothing),
    ("meta/registered_spec", Takes::Nothing),
    ("meta/completion_cursor", Takes::Nothing),
    ("meta/wait_for_completion", Takes::Nothing),
    ("meta/retire_member", Takes::Nothing),
    ("meta/respawn", Takes::Nothing),
    ("meta/respawn_member", Takes::Nothing),
    ("meta/reload_member", Takes::Nothing),
    ("meta/reset", Takes::Nothing),
    ("meta/delete_identity", Takes::Nothing),
    ("meta/force_cancel_member", Takes::Nothing),
    ("meta/stop_member_run", Takes::Nothing),
    ("meta/console/send", Takes::TheTwoValues),
    ("meta/console/recover_send", Takes::Nothing),
    ("meta/console/query_timeline", Takes::TheTwoValues),
    ("meta/schedule/list", Takes::Nothing),
    ("meta/schedule/update", Takes::Nothing),
    ("meta/schedule/pause", Takes::Nothing),
    ("meta/schedule/resume", Takes::Nothing),
    ("meta/schedule/delete", Takes::Nothing),
    ("meta/schedule/create", Takes::Nothing),
    ("meta/agent_memory/remember", Takes::Nothing),
    ("meta/agent_memory/update", Takes::Nothing),
    ("meta/agent_memory/forget", Takes::Nothing),
    ("meta/agent_memory/recall", Takes::Nothing),
    ("meta/agent_memory/manifest", Takes::Nothing),
    ("meta/memory/panel/records", Takes::Nothing),
    ("meta/memory/quarantine/decide", Takes::Nothing),
    ("meta/mob_events/query", Takes::Nothing),
    ("meta/mob_events/subscribe", Takes::Nothing),
    ("meta/cross_mob/peer_info", Takes::Nothing),
    ("meta/collect_completed", Takes::Nothing),
    ("meta/cancel_flow", Takes::Nothing),
    ("meta/tool_grants/grant", Takes::Nothing),
    ("meta/tool_grants/list", Takes::Nothing),
    ("meta/tool_grants/audit", Takes::Nothing),
    ("meta/tool_grants/revoke", Takes::Nothing),
    ("meta/retire", Takes::TheTwoValues),
];

/// Every method of one member takes `member_mob_id` or refuses it; none
/// ignores it. Each method of [`METHODS_OF_ONE_MEMBER`] is asked with a
/// `member_mob_id` that names a mob the host does not have. A method that
/// takes the two values is asked for the launch mob's member under that
/// mob and answers what it answers a name no member holds, the names
/// apart: never the launch mob's member. A read of the roster of the
/// door's own mob refuses the other mob. Every other method refuses the
/// field, whether or not this console serves the method. Each method of
/// the list is one the door's table of member fields reads its member
/// from `identity` first. Every method of the door that is not on the
/// list ([`console_methods`]: the methods that name a member in another
/// way and the methods that name none) refuses the field too, before
/// anything else of the request is read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_method_of_one_member_takes_member_mob_id_or_refuses_it() -> TestResult {
    let nested = Nested::start("member-mob-table").await?;
    // The launch mob's member has a message, so its page is not the page
    // of a name no member holds.
    let (status, sent) = send_on_console(&nested.app, &nested.owner, LAUNCH_MEMBER).await;
    if !status.is_success() {
        return Err(format!("the owner's send to the launch mob's member: {status} {sent}").into());
    }
    let apart = |answer: &Value| {
        nested
            .apart_from_the_names(answer)
            .replace(LAUNCH_MEMBER, "<member>")
    };
    let mut wrong = Vec::new();
    for (method, takes) in METHODS_OF_ONE_MEMBER {
        let first_field = console_method_member_fields(method)
            .and_then(|fields| fields.first())
            .map(|field| field.path);
        if first_field != Some(&["identity"][..]) {
            wrong.push(format!(
                "{method} is not a method the door reads one member of from `identity`"
            ));
        }
        let field = READS_OF_ONE_MEMBER
            .iter()
            .find(|(read, _)| read == method)
            .map_or("identity", |(_, field)| *field);
        let request = |member: &str, key: &str| {
            let mut params = json!({ field: member });
            if *method == "meta/console/send" {
                params["content"] = json!(content_of(key));
                params["origin"] = json!("console");
                params["idempotency_key"] = json!(key);
            }
            params
        };
        match takes {
            Takes::Nothing => {
                let answer = nested
                    .ask_within(
                        method,
                        json!({
                            "identity": "nobody",
                            "member_id": "nobody",
                            "member_mob_id": nested.nowhere(),
                        }),
                    )
                    .await;
                if !is_the_not_accepted_refusal(&answer, method) {
                    wrong.push(format!("{method} does not refuse the field: {answer}"));
                }
            }
            Takes::TheOwnMobAlone => {
                let mut params = request(LAUNCH_MEMBER, "elsewhere");
                params["member_mob_id"] = json!(nested.nowhere());
                let answer = nested.ask_within(method, params).await;
                if !is_the_roster_refusal(&answer, method) {
                    wrong.push(format!("{method} does not refuse the other mob: {answer}"));
                }
            }
            Takes::TheTwoValues => {
                let absent = nested.ask_within(method, request("nobody", "absent")).await;
                let mut params = request(LAUNCH_MEMBER, "elsewhere");
                params["member_mob_id"] = json!(nested.nowhere());
                let answer = nested.ask_within(method, params).await;
                if apart(&answer) != apart(&absent) {
                    wrong.push(format!(
                        "{method} does not answer a member of a mob the host does not have as \
                         an absent member: {answer}\n  a name no member holds: {absent}"
                    ));
                }
            }
        }
    }
    // The door's other methods. The sweep stops at twelve that do not
    // refuse the field.
    let mut unrefused = 0;
    for method in console_methods() {
        if METHODS_OF_ONE_MEMBER
            .iter()
            .any(|(listed, _)| *listed == method)
        {
            continue;
        }
        if unrefused == 12 {
            wrong.push("more of the door's other methods were not asked".to_string());
            break;
        }
        let answer = nested
            .ask_within(
                &method,
                json!({
                    "identity": "nobody",
                    "member_id": "nobody",
                    "member_mob_id": nested.nowhere(),
                }),
            )
            .await;
        if !is_the_not_accepted_refusal(&answer, &method) {
            unrefused += 1;
            let answer = answer.to_string().chars().take(300).collect::<String>();
            wrong.push(format!("{method} does not refuse the field: {answer}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a method of one member neither takes `member_mob_id` nor refuses it:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A fork under the one string that names another person's nested member
/// is answered as a fork under a name that cannot be given: an unknown
/// identity, in the door's words for a name the caller's request may not
/// record under.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_under_the_string_of_another_persons_nested_member_is_answered_an_unknown_identity()
-> TestResult {
    let nested = Nested::start("member-mob-fork").await?;
    let one_string = nested.nested_by_one_string();
    let forked =
        fork_on_console(&nested.app, Some(&nested.other), LAUNCH_MEMBER, &one_string).await;
    assert_eq!(
        forked["error"],
        json!({ "code": -32001, "message": format!("unknown identity: {one_string}") }),
        "{forked}"
    );
    assert!(
        nested
            .owner_inspects(json!({ "identity": one_string }))
            .await,
        "the owner is still answered its nested member"
    );
    Ok(())
}

/// Two members of ONE mob under one alias are refused as before. A console
/// with an identity runtime answers invalid params with the kind
/// `ambiguous_live_identity_alias`, the identity the request named and the
/// two candidates, on each method that refuses so. On the methods that
/// take `member_mob_id`, naming that mob does not choose between them: the
/// same answer, byte for byte (a method that does not take the field
/// refuses it before it reads the alias). A console without an identity
/// runtime answers the two request forms the same bytes too, and no
/// member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_members_of_one_mob_under_one_alias_are_refused_as_before() -> TestResult {
    use super::console_private_audience_tests::fully_composed_console;
    use super::console_token_lapse_tests::console_rpc_as;

    let nested = Nested::start_without_the_nested_mob("member-mob-alias").await?;
    let alias = "review:singleton";
    let members = ["rt:review:singleton:0", "rt:review:singleton:1"];
    for runtime_id in members {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("agent_identity".to_string(), alias.to_string());
        nested
            .runtime
            .handle()
            .spawn_spec(
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    crate::member_comms_id::mob_member_id_str(runtime_id).into_owned(),
                    Some("You are one of two members under one alias.".into()),
                    None,
                    None,
                )
                .with_labels(labels),
            )
            .await?;
    }
    let memory_dir = tempfile::tempdir()?;
    let with_identities = console_json_router_with_state(
        fully_composed_console(&nested.runtime, LAUNCH_MEMBER, memory_dir.path()).await?,
    );
    let mut wrong = Vec::new();
    for method in [
        "meta/status_identity",
        "meta/inspect_identity",
        "meta/retire",
        "meta/respawn",
        "meta/reset",
    ] {
        let by_alias = json!({ "identity": alias });
        let by_alias_and_mob = json!({ "identity": alias, "member_mob_id": nested.launch_mob });
        let typed = console_rpc_as(
            &with_identities,
            Some(&nested.owner),
            method,
            by_alias.clone(),
        )
        .await;
        let data = &typed["error"]["data"];
        let mut candidates = data["candidates"]
            .as_array()
            .map(|candidates| {
                candidates
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        candidates.sort_unstable();
        if typed["error"]["code"] != json!(-32602)
            || data["kind"] != json!("ambiguous_live_identity_alias")
            || data["identity"] != json!(alias)
            || candidates != members
            || data.as_object().map(serde_json::Map::len) != Some(3)
            || !typed["result"].is_null()
        {
            wrong.push(format!("{method} by the alias: {typed}"));
        }
        if !matches!(
            method,
            "meta/status_identity" | "meta/inspect_identity" | "meta/retire"
        ) {
            continue;
        }
        let with_the_mob = console_rpc_as(
            &with_identities,
            Some(&nested.owner),
            method,
            by_alias_and_mob.clone(),
        )
        .await;
        if with_the_mob != typed {
            wrong.push(format!(
                "{method} by the alias and the mob: {with_the_mob}\n  by the alias: {typed}"
            ));
        }
        let plain = nested.ask(method, by_alias).await;
        let plain_with_the_mob = nested.ask(method, by_alias_and_mob).await;
        if plain["error"].is_null() || plain_with_the_mob != plain {
            wrong.push(format!(
                "{method} on a console without an identity runtime, by the alias: {plain}\n  \
                 by the alias and the mob: {plain_with_the_mob}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "two members of one mob under one alias are not refused as before:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// No memory method takes a string of a mob and a name: an identity a
/// memory method names holds no `/`. Asked by the one string that another
/// person's member of the launch mob and the owner's nested member spell,
/// each memory method answers both persons invalid params, the same bytes,
/// and nothing of any member's memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_memory_method_takes_no_string_of_a_mob_and_a_name() -> TestResult {
    use super::console_private_audience_tests::fully_composed_console;
    use super::console_token_lapse_tests::console_rpc_as;

    let nested = Nested::start_without_the_nested_mob("member-mob-memory").await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let one_string = nested.nested_by_one_string();
    let memory_dir = tempfile::tempdir()?;
    let app = console_json_router_with_state(
        fully_composed_console(&nested.runtime, LAUNCH_MEMBER, memory_dir.path()).await?,
    );
    let mut wrong = Vec::new();
    for (method, params) in [
        (
            "meta/agent_memory/remember",
            json!({
                "identity": one_string,
                "title": "The heron",
                "body": "The heron is at the north pond.",
                "tags": ["heron"],
            }),
        ),
        (
            "meta/agent_memory/recall",
            json!({
                "identity": one_string,
                "selection": "contextual",
                "query_text": "Where is the heron?",
                "query_terms": ["heron"],
            }),
        ),
        (
            "meta/agent_memory/manifest",
            json!({ "identity": one_string }),
        ),
        (
            "meta/agent_memory/forget",
            json!({ "identity": one_string, "memory_id": "any-memory" }),
        ),
    ] {
        let others = console_rpc_as(&app, Some(&nested.other), method, params.clone()).await;
        let owners = console_rpc_as(&app, Some(&nested.owner), method, params).await;
        if others["error"]["code"] != json!(-32602)
            || !others["result"].is_null()
            || owners != others
        {
            wrong.push(format!(
                "{method}: the other person: {others}\n  the owner: {owners}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a memory method takes a string of a mob and a name:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The send route takes no `member_mob_id`. For a caller who is answered
/// for two members of one spelling, a send by the route under that one
/// string is taken by the launch mob's member, as before: on the session
/// the inspect answers a person who is answered for that member alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_send_route_by_one_string_two_members_spell_reaches_the_launch_mobs_member()
-> TestResult {
    let nested = Nested::start("member-mob-route").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let launch_session = nested
        .session_of(&nested.other, json!({ "identity": one_string }))
        .await;
    let sent = nested
        .send_by_the_route_as(&nested.owner, "by-the-route")
        .await;
    let taken = sent
        .as_ref()
        .is_ok_and(|sent| !launch_session.is_null() && sent["session_id"] == launch_session);
    assert!(
        taken,
        "the owner's send by the route is not taken on the session {launch_session} of the \
         launch mob's member: {sent:?}"
    );
    Ok(())
}

impl Nested {
    /// The console of `runtime`: sign-in on, and an aggregator that knows
    /// the runtime under no namespace.
    fn console_of(runtime: &MobRuntime) -> Router {
        let mut console = enforced_console(runtime, None);
        console.visibility_policy = Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy);
        let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
        // The console is told its default mob, as a host tells it.
        aggregator.tell_default_mob(crate::member_key::DefaultMob::new(runtime.mob_id().clone()));
        aggregator.register_runtime_handles_with_policy(
            "default",
            "",
            runtime.clone(),
            None,
            ConsoleEventStore::new(),
            Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
        );
        console.console_aggregator = Some(aggregator);
        console_json_router_with_state(console)
    }

    /// The launch mob's member under the one string ends: it is retired
    /// and leaves its roster.
    async fn end_the_namesake(&self) -> TestResult {
        let one_string = self.nested_by_one_string();
        let identity =
            AgentIdentity::from(crate::member_comms_id::mob_member_id_str(&one_string).as_ref());
        let handle = self.runtime.handle();
        handle.retire(identity.clone()).await?;
        for _ in 0..100 {
            if handle.get_member(&identity).await?.is_none() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err("the fixture: the launch mob's member did not leave its roster".into())
    }

    /// What a row of ended members holds once one member of the one
    /// spelling has ended with the message `to-the-ended` and the other
    /// lives with the message `to-the-live`: the timeline by the one
    /// string is refused, each member's page by its two values holds its
    /// own message alone, and a send by the one string is taken on the
    /// live member's session.
    async fn ended_and_live(&self, ended: Value, live: Value) -> Vec<String> {
        let one_string = self.nested_by_one_string();
        let mut wrong = Vec::new();
        let page = self
            .ask(
                "meta/console/query_timeline",
                json!({ "identity": one_string }),
            )
            .await;
        if !is_the_shared_spelling_refusal(&page, &one_string) {
            wrong.push(format!("the timeline by the one string: {page}"));
        }
        for (who, member, own, others) in [
            ("the ended member", &ended, "to-the-ended", "to-the-live"),
            ("the live member", &live, "to-the-live", "to-the-ended"),
        ] {
            if !self.page_holds(&self.owner, member.clone(), own).await {
                wrong.push(format!(
                    "the page of {who} by its two values does not hold its message"
                ));
            }
            if self.page_holds(&self.owner, member.clone(), others).await {
                wrong.push(format!(
                    "the page of {who} by its two values holds the other member's message"
                ));
            }
        }
        let session = self.session_of(&self.owner, live).await;
        let sent = self
            .send_as(&self.owner, json!({ "identity": one_string }), "again")
            .await;
        if session.is_null() || !sent["error"].is_null() || sent["result"]["session_id"] != session
        {
            wrong.push(format!(
                "a send by the one string is not taken on the live member's session {session}: \
                 {sent}"
            ));
        }
        wrong
    }
}

/// AN ENDED MEMBER OF THE LAUNCH MOB beside a live member of the nested
/// mob, of one spelling. The ended member's messages can still be read:
/// a timeline read by the one string is refused, and each member's page
/// is read by its two values. An ended member has no record to inspect,
/// so the inspect by the one string answers the live member; and a send
/// by the one string is taken by the live member, the only one that can
/// take it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_by_one_string_of_an_ended_launch_member_and_a_live_nested_one_is_refused()
-> TestResult {
    let nested = Nested::start_without_the_nested_mob("member-mob-ended").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    nested
        .send_by_the_route_as(&nested.owner, "to-the-ended")
        .await
        .map_err(|refused| {
            format!("the fixture: the launch mob's member takes no send: {refused}")
        })?;
    nested.end_the_namesake().await?;
    let mut wrong = Vec::new();
    let ended_alone = nested
        .ask(
            "meta/console/inspect_identity",
            json!({ "identity": one_string }),
        )
        .await;
    if ended_alone["error"]["code"] != json!(-32001) {
        wrong.push(format!(
            "the inspect answers the ended member a record: {ended_alone}"
        ));
    }
    nested.create_the_nested_mob().await?;
    nested
        .send_by_the_route_as(&nested.owner, "to-the-live")
        .await
        .map_err(|refused| format!("the fixture: the nested member takes no send: {refused}"))?;
    let inspected = nested
        .ask(
            "meta/console/inspect_identity",
            json!({ "identity": one_string }),
        )
        .await;
    if inspected["result"]["identity"]["labels"]["source_mob_id"] != json!(nested.team) {
        wrong.push(format!(
            "the inspect by the one string does not answer the live member: {inspected}"
        ));
    }
    wrong.extend(
        nested
            .ended_and_live(
                json!({ "identity": one_string, "member_mob_id": nested.launch_mob }),
                json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team }),
            )
            .await,
    );
    assert!(
        wrong.is_empty(),
        "an ended member of the launch mob beside a live nested member of its spelling:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// AN ENDED MEMBER OF THE NESTED MOB beside a live member of the launch
/// mob, of one spelling: the same the other way round. The timeline by the
/// one string is refused, each member's page is read by its two values,
/// and a send by the one string is taken by the live member of the launch
/// mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_by_one_string_of_an_ended_nested_member_and_a_live_launch_one_is_refused()
-> TestResult {
    let nested = Nested::start("member-mob-ended-nested").await?;
    let one_string = nested.nested_by_one_string();
    nested
        .send_by_the_route_as(&nested.owner, "to-the-ended")
        .await
        .map_err(|refused| format!("the fixture: the nested member takes no send: {refused}"))?;
    let retired = nested
        .ask("meta/retire", json!({ "identity": one_string }))
        .await;
    if !retired["error"].is_null() {
        return Err(format!("the fixture: the nested member is not retired: {retired}").into());
    }
    let mut ended = false;
    for _ in 0..50 {
        if !nested
            .owner_inspects(json!({ "identity": one_string }))
            .await
        {
            ended = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if !ended {
        return Err("the fixture: the retired nested member is still inspected".into());
    }
    nested.seat_the_namesake().await?;
    nested
        .send_by_the_route_as(&nested.owner, "to-the-live")
        .await
        .map_err(|refused| {
            format!("the fixture: the launch mob's member takes no send: {refused}")
        })?;
    let wrong = nested
        .ended_and_live(
            json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team }),
            json!({ "identity": one_string, "member_mob_id": nested.launch_mob }),
        )
        .await;
    assert!(
        wrong.is_empty(),
        "an ended member of the nested mob beside a live launch member of its spelling:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A CONSOLE WITHOUT A RUNTIME OF ITS OWN serves its runtimes through its
/// aggregator and names a member with its runtime's namespace
/// (`<namespace>/<identity>`). It takes `member_mob_id` on no method: a
/// request that carries the field is refused, also on each method that
/// takes the field on a console with a runtime of its own, whether it
/// names the launch mob's member or the nested member, with the namespace
/// or without it. Its members are named by the one string.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_console_without_a_runtime_of_its_own_takes_member_mob_id_on_no_method() -> TestResult {
    use super::console_token_lapse_tests::console_rpc_as;

    let nested = Nested::start("member-mob-namespace").await?;
    let console = enforced_console(&nested.runtime, None);
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "ns",
        nested.runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );
    let app = super::console_json_router_with_aggregator_and_access(
        console.decisions,
        aggregator,
        console.access,
    );
    let named = format!("ns/{LAUNCH_MEMBER}");
    let inspect = "meta/console/inspect_identity";
    let by_string = console_rpc_as(
        &app,
        Some(&nested.owner),
        inspect,
        json!({ "identity": named }),
    )
    .await;
    if !by_string["error"].is_null() {
        return Err(format!(
            "the fixture: the console does not answer its member by the namespace and its \
             identity: {by_string}"
        )
        .into());
    }
    let nested_named = format!("ns/{}", nested.nested_by_one_string());
    let mut wrong = Vec::new();
    for (method, takes) in METHODS_OF_ONE_MEMBER {
        if *takes == Takes::Nothing {
            continue;
        }
        for (name, mob) in [
            (named.as_str(), &nested.launch_mob),
            (LAUNCH_MEMBER, &nested.launch_mob),
            (nested_named.as_str(), &nested.team),
            (NESTED_MEMBER, &nested.team),
        ] {
            let answer = console_rpc_as(
                &app,
                Some(&nested.owner),
                method,
                json!({
                    "identity": name,
                    "member_id": name,
                    "member_mob_id": mob,
                    "content": content_of("refused"),
                    "origin": "console",
                    "idempotency_key": "refused",
                }),
            )
            .await;
            if !is_the_not_accepted_refusal(&answer, method) {
                wrong.push(format!(
                    "{method}, the name {name} with the mob {mob}: {answer}"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a console without a runtime of its own does not refuse `member_mob_id`:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The door's list of the methods that take `member_mob_id` is sorted,
/// names no method twice, and is the methods of [`METHODS_OF_ONE_MEMBER`]
/// that take the two values or their own mob alone.
#[test]
fn the_doors_list_of_methods_that_take_member_mob_id_is_sorted_and_is_the_tables() {
    let listed = super::CONSOLE_METHODS_TAKING_MEMBER_MOB_ID;
    assert!(
        listed.windows(2).all(|pair| pair[0] < pair[1]),
        "the list is not sorted, or names a method twice: {listed:?}"
    );
    let mut of_the_table = METHODS_OF_ONE_MEMBER
        .iter()
        .filter(|(_, takes)| *takes != Takes::Nothing)
        .map(|(method, _)| *method)
        .collect::<Vec<_>>();
    of_the_table.sort_unstable();
    assert_eq!(listed, of_the_table.as_slice());
}

/// Every method the console's door names: the method literals of its
/// dispatch and of the files it serves families from, and the methods it
/// names by constant.
fn console_methods() -> std::collections::BTreeSet<String> {
    fn literals(source: &str, out: &mut std::collections::BTreeSet<String>) {
        for prefix in ["\"meta/", "\"live/"] {
            let mut rest = source;
            while let Some(at) = rest.find(prefix) {
                let tail = &rest[at + 1..];
                let end = tail.find('"').unwrap_or(0);
                let method = &tail[..end];
                if !method.is_empty()
                    && !method.ends_with('/')
                    && method
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_/".contains(c))
                {
                    out.insert(method.to_string());
                }
                rest = &tail[end.max(1)..];
            }
        }
    }
    let mut methods = std::collections::BTreeSet::new();
    for source in [
        include_str!("http_console.rs"),
        include_str!("console_voice.rs"),
        include_str!("http_console/shared_panels.rs"),
        include_str!("http_console/schedules.rs"),
        include_str!("http_console/audit.rs"),
        include_str!("http_console/mcp_apps.rs"),
    ] {
        literals(source, &mut methods);
    }
    methods.extend(
        [
            crate::rpc::CONSOLE_WHOAMI_METHOD,
            crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
            crate::rpc::CONSOLE_AUDIT_METHOD,
        ]
        .map(str::to_string),
    );
    methods.extend(
        crate::rpc::workgraph_methods::WORKGRAPH_READ_METHODS
            .iter()
            .chain(crate::rpc::workgraph_methods::WORKGRAPH_MUTATE_METHODS)
            .chain(crate::rpc::workgraph_methods::WORKGRAPH_CONSOLE_MUTATE_METHODS)
            .chain(crate::mob_announcements::rpc::ANNOUNCEMENT_METHODS)
            .chain(crate::rpc::mob_methods::MEMBER_DECLARATION_METHODS)
            .map(|method| (*method).to_string()),
    );
    methods.extend(
        [
            crate::rpc::topology_methods::TOPOLOGY_QUERY_METHOD,
            crate::rpc::topology_methods::TOPOLOGY_PLAN_METHOD,
            crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD,
            crate::rpc::topology_methods::TOPOLOGY_OPERATION_METHOD,
            crate::rpc::topology_methods::TOPOLOGY_AUDIT_METHOD,
        ]
        .map(str::to_string),
    );
    methods
}

/// The methods of [`METHODS_OF_ONE_MEMBER`] that refuse the field and end
/// or change the member they reach, or something of it. The respawn, the
/// reset and the retire are last, in that order.
const METHODS_THAT_CHANGE_A_MEMBER: &[&str] = &[
    "meta/identity/warm",
    "meta/reload_member",
    "meta/respawn_member",
    "meta/delete_identity",
    "meta/force_cancel_member",
    "meta/stop_member_run",
    "meta/console/recover_send",
    "meta/schedule/create",
    "meta/schedule/update",
    "meta/schedule/pause",
    "meta/schedule/resume",
    "meta/schedule/delete",
    "meta/agent_memory/remember",
    "meta/agent_memory/update",
    "meta/agent_memory/forget",
    "meta/memory/quarantine/decide",
    "meta/mob_events/subscribe",
    "meta/collect_completed",
    "meta/cancel_flow",
    "meta/tool_grants/grant",
    "meta/tool_grants/revoke",
    "meta/respawn",
    "meta/reset",
    "meta/retire_member",
];

/// The request a method that refuses `member_mob_id` is asked with for the
/// member `name`: the name in the field the method reads its member from,
/// and what the method needs beside it so that it comes as far as its
/// member: the run, the input, the send, the schedule, the record or the
/// grant it also names (none of them exists), or the words it is given. A
/// method that refuses a field it does not know is given none.
fn request_of(method: &str, name: &str) -> Value {
    const NO_SUCH: &str = "00000000-0000-4000-8000-000000000001";
    let by_identity_alone = |more: Value| {
        let mut params = more;
        params["identity"] = json!(name);
        params
    };
    let mut params = match method {
        // These refuse a field they do not know.
        "meta/console/recover_send" => {
            return by_identity_alone(
                json!({ "origin": "console", "idempotency_key": "never-sent" }),
            );
        }
        "meta/tool_grants/grant" => {
            return by_identity_alone(
                json!({ "target": { "kind": "tool", "name": "no-such-tool" } }),
            );
        }
        "meta/tool_grants/list" | "meta/tool_grants/audit" => {
            return by_identity_alone(json!({}));
        }
        // A grant is revoked by its id: the method names no member.
        "meta/tool_grants/revoke" => return json!({ "grant_id": NO_SUCH }),
        "meta/stop_member_run" => json!({ "run_id": NO_SUCH, "reason": "a row asks" }),
        "meta/cancel_flow" => json!({ "run_id": NO_SUCH }),
        "meta/send_status" => json!({ "input_id": NO_SUCH }),
        "meta/schedule/create" => json!({
            "content": "a word.",
            "trigger": { "type": "once", "due_at_utc": "2099-01-01T00:00:00Z" },
        }),
        "meta/schedule/update"
        | "meta/schedule/pause"
        | "meta/schedule/resume"
        | "meta/schedule/delete" => json!({ "schedule_id": NO_SUCH }),
        "meta/agent_memory/remember" => json!({ "title": "a title", "body": "a body." }),
        "meta/agent_memory/update" => {
            json!({ "memory_id": NO_SUCH, "title": "a title", "body": "a body." })
        }
        "meta/agent_memory/forget" => json!({ "memory_id": NO_SUCH }),
        _ => json!({}),
    };
    params["identity"] = json!(name);
    params["member_id"] = json!(name);
    params
}

/// A method that refuses `member_mob_id` reads one string that two members
/// spell as the launch mob's member, as before: it cannot ask for a field
/// it refuses, so the nested member of a shared spelling is not reached
/// through it. The owner, who is answered for both members, asks every
/// such method of [`METHODS_OF_ONE_MEMBER`] by the one string, with what
/// the method needs beside the name ([`request_of`]). A method that
/// changes nothing is asked before the nested mob exists and after, and
/// answers the same bytes both times (`meta/wait_for_completion` is left
/// out: it waits, and has no answer to compare). A method that changes a
/// member ([`METHODS_THAT_CHANGE_A_MEMBER`]) is asked once the nested mob
/// exists: it never asks for the field, and the nested member keeps its
/// seat. The row prints each answer, so that a run shows which methods
/// answer a result and which are refused, and by what.
///
/// This row asks a console without an identity runtime: there the respawn
/// and the reset move the launch mob's member to another session, and the
/// retire ends it. `meta/delete_identity` stands before them and ends
/// nothing there: that console refuses it for want of an identity runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_method_that_refuses_the_field_reads_the_one_string_as_the_launch_mobs_member()
-> TestResult {
    the_refusing_methods_by_the_one_string("member-mob-unreached", false).await
}

/// [`a_method_that_refuses_the_field_reads_the_one_string_as_the_launch_mobs_member`]
/// on a console with an identity runtime, which holds the launch mob's
/// first member and not the member of the shared spelling: no method asks
/// for the field, a method that changes nothing answers the same bytes
/// before the nested mob exists and after, and the nested member keeps its
/// seat through every method that changes a member. An identity runtime
/// takes no name that holds `/`, so there the respawn and the reset by the
/// one string are refused as an invalid identity and move no member (so
/// are the warm and the delete); the retire by the roster ends the launch
/// mob's member as on the other console.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_method_that_refuses_the_field_reads_the_one_string_so_with_an_identity_runtime()
-> TestResult {
    the_refusing_methods_by_the_one_string("member-mob-unreached-identities", true).await
}

/// The body of the two rows above, on a console without an identity
/// runtime or with one.
async fn the_refusing_methods_by_the_one_string(
    tag: &str,
    with_an_identity_runtime: bool,
) -> TestResult {
    use super::console_token_lapse_tests::console_rpc_as;

    /// The answer of `method` on `app` for the holder of `bearer`, given
    /// up after ten seconds.
    async fn asked(app: &Router, bearer: &str, method: &str, params: Value) -> Value {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            console_rpc_as(app, Some(bearer), method, params),
        )
        .await
        .unwrap_or_else(|_| json!({ "no answer within ten seconds": method }))
    }

    /// The session the inspect on `app` answers the holder of `bearer` for
    /// the member `member` names; null where it answers no member.
    async fn session_on(app: &Router, bearer: &str, member: &Value) -> Value {
        let answer = asked(app, bearer, "meta/console/inspect_identity", member.clone()).await;
        answer["result"]["identity"]["session_id"].clone()
    }

    let nested = Nested::start_without_the_nested_mob(tag).await?;
    nested.seat_the_namesake().await?;
    let memory_dir = tempfile::tempdir()?;
    let (console, app) = if with_an_identity_runtime {
        (
            "with an identity runtime",
            console_json_router_with_state(
                super::console_private_audience_tests::fully_composed_console(
                    &nested.runtime,
                    LAUNCH_MEMBER,
                    memory_dir.path(),
                )
                .await?,
            ),
        )
    } else {
        ("without an identity runtime", nested.app.clone())
    };
    let one_string = nested.nested_by_one_string();
    let request = |method: &str| request_of(method, &one_string);
    let shown = |answer: &Value| answer.to_string().chars().take(400).collect::<String>();
    let unchanging = METHODS_OF_ONE_MEMBER
        .iter()
        .filter(|(method, takes)| {
            *takes == Takes::Nothing
                && *method != "meta/wait_for_completion"
                && !METHODS_THAT_CHANGE_A_MEMBER.contains(method)
        })
        .map(|(method, _)| *method)
        .collect::<Vec<_>>();
    let mut before = Vec::new();
    for method in &unchanging {
        before.push(asked(&app, &nested.owner, method, request(method)).await);
    }
    nested.create_the_nested_mob().await?;
    let asks_for_the_field =
        |answer: &Value| answer["error"]["data"]["kind"] == json!("member_mob_id_required");
    let mut wrong = Vec::new();
    for (method, before) in unchanging.iter().zip(before) {
        let after = asked(&app, &nested.owner, method, request(method)).await;
        eprintln!("{console}: {method}: {}", shown(&after));
        if asks_for_the_field(&after) || after != before {
            wrong.push(format!(
                "{method}: once the nested mob exists: {after}\n  before: {before}"
            ));
        }
    }
    // The other person is answered for the launch mob's member alone: its
    // inspect names that member's session.
    let launch_member = json!({ "identity": one_string });
    for method in METHODS_THAT_CHANGE_A_MEMBER {
        let session = session_on(&app, &nested.other, &launch_member).await;
        if !METHODS_OF_ONE_MEMBER
            .iter()
            .any(|(listed, takes)| listed == method && *takes == Takes::Nothing)
        {
            wrong.push(format!(
                "{method} is not a method of the list that refuses the field"
            ));
        }
        let answer = asked(&app, &nested.owner, method, request(method)).await;
        eprintln!("{console}: {method}: {}", shown(&answer));
        if asks_for_the_field(&answer) {
            wrong.push(format!("{method} asks for the field: {answer}"));
        }
        if !nested.nested_member_is_seated().await? {
            wrong.push(format!(
                "{method} by the one string unseated the nested member"
            ));
        }
        if matches!(*method, "meta/respawn" | "meta/reset") {
            let moved = session_on(&app, &nested.other, &launch_member).await;
            eprintln!("{console}: {method}: the launch mob's member: {session} then {moved}");
            let as_it_should = if with_an_identity_runtime {
                // An identity runtime takes no name that holds `/`: the
                // method is refused and moves no member.
                answer["error"]["code"] == json!(-32602) && !moved.is_null() && moved == session
            } else {
                answer["error"].is_null() && !moved.is_null() && moved != session
            };
            if !as_it_should {
                wrong.push(format!(
                    "{method} by the one string: the launch mob's member was on the session \
                     {session} and is on {moved}: {answer}"
                ));
            }
        }
    }
    let namesake =
        AgentIdentity::from(crate::member_comms_id::mob_member_id_str(&one_string).as_ref());
    let mut ended = false;
    for _ in 0..100 {
        if nested
            .runtime
            .handle()
            .get_member(&namesake)
            .await?
            .is_none()
        {
            ended = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    eprintln!("{console}: the launch mob's member ended: {ended}");
    if !ended {
        wrong.push("the retire by the one string does not end the launch mob's member".to_string());
    }
    assert!(
        wrong.is_empty(),
        "a method that refuses the field does not read the one string as the launch mob's \
         member, on a console {console}:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// Two members of ONE mob under one alias are an error, whoever asks: a
/// caller answered for one of them is refused as
/// `two_members_of_one_mob_under_one_alias_are_refused_as_before` pins.
/// Two members of the launch mob carry one alias, written by an agent's
/// label; one of them is another person's own. The owner asks each read
/// that answers a record by the alias, on a console without an identity
/// runtime and on one with it: each read is refused and answers no record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_answered_for_one_of_two_members_under_one_alias_is_refused_as_for_both()
-> TestResult {
    let nested = Nested::start_without_the_nested_mob("member-mob-alias-one").await?;
    let alias = "review:pair";
    let shown = "rt:review:pair:shown";
    let kept = "rt:review:pair:kept";
    let others_scope = nested
        .ask_as(&nested.other, crate::rpc::CONSOLE_WHOAMI_METHOD, json!({}))
        .await["result"]["caller_scope"]
        .as_str()
        .ok_or("the other person has a caller scope")?
        .to_string();
    for (runtime_id, private_to) in [(shown, None), (kept, Some(others_scope.as_str()))] {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("agent_identity".to_string(), alias.to_string());
        if let Some(person) = private_to {
            labels.insert(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                person.to_string(),
            );
        }
        nested
            .runtime
            .handle()
            .spawn_spec(
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    crate::member_comms_id::mob_member_id_str(runtime_id).into_owned(),
                    Some("You are one of two members under one alias.".into()),
                    None,
                    None,
                )
                .with_labels(labels),
            )
            .await?;
    }
    let memory_dir = tempfile::tempdir()?;
    let with_identities = console_json_router_with_state(
        super::console_private_audience_tests::fully_composed_console(
            &nested.runtime,
            LAUNCH_MEMBER,
            memory_dir.path(),
        )
        .await?,
    );
    let mut wrong = Vec::new();
    for (console, app) in [
        ("a console without an identity runtime", &nested.app),
        ("a console with an identity runtime", &with_identities),
    ] {
        for (method, field) in READS_THAT_ANSWER_A_RECORD {
            let answer = super::console_token_lapse_tests::console_rpc_as(
                app,
                Some(&nested.owner),
                method,
                json!({ *field: alias }),
            )
            .await;
            if answer["error"].is_null() || !answer["result"].is_null() {
                wrong.push(format!("{method} on {console}: {answer}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a caller answered for one of two members of one mob under one alias is not \
         refused:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The two values tell a caller nothing of a member it is not answered
/// for, also where a member of the launch mob that every caller is
/// answered for has the one string as its name. Another signed-in person
/// names the owner's nested member by the mob it sits in and its name
/// there: each read answers what it answers that person for a mob the host
/// does not have and a name no member holds, the names apart; a send by
/// the two values is answered so and the nested member takes no message;
/// a retire by them is answered so and the nested member keeps its seat.
/// By the one string that person is answered the launch mob's member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_values_tell_nothing_of_a_member_beside_a_launch_member_of_its_spelling()
-> TestResult {
    let nested = Nested::start("member-mob-hidden-pair").await?;
    nested.seat_the_namesake().await?;
    let the_nested = |field: &str| json!({ field: NESTED_MEMBER, "member_mob_id": nested.team });
    let nobody = |field: &str| json!({ field: "nobody", "member_mob_id": nested.nowhere() });
    let mut wrong = Vec::new();
    for (method, field) in READS_OF_ONE_MEMBER {
        let hidden = nested
            .ask_as(&nested.other, method, the_nested(field))
            .await;
        let absent = nested.ask_as(&nested.other, method, nobody(field)).await;
        if nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent) {
            wrong.push(format!(
                "{method}: the hidden member: {hidden}\n  no member: {absent}"
            ));
        }
    }
    let hidden = nested
        .send_as(&nested.other, the_nested("identity"), "hidden")
        .await;
    let absent = nested
        .send_as(&nested.other, nobody("identity"), "absent")
        .await;
    if nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent) {
        wrong.push(format!(
            "meta/console/send: the hidden member: {hidden}\n  no member: {absent}"
        ));
    }
    if nested
        .page_holds(&nested.owner, the_nested("identity"), "hidden")
        .await
    {
        wrong.push("the nested member took the other person's send".to_string());
    }
    let hidden = nested
        .ask_as(&nested.other, "meta/retire", the_nested("identity"))
        .await;
    let absent = nested
        .ask_as(&nested.other, "meta/retire", nobody("identity"))
        .await;
    if nested.apart_from_the_names(&hidden) != nested.apart_from_the_names(&absent) {
        wrong.push(format!(
            "meta/retire: the hidden member: {hidden}\n  no member: {absent}"
        ));
    }
    if !nested.nested_member_stays_seated().await? {
        wrong.push("the nested member lost its seat to the other person's retire".to_string());
    }
    let by_string = nested
        .ask_as(
            &nested.other,
            "meta/console/inspect_identity",
            json!({ "identity": nested.nested_by_one_string() }),
        )
        .await;
    if !by_string["error"].is_null() {
        wrong.push(format!(
            "the other person is not answered the launch mob's member by the one string: \
             {by_string}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "the two values tell a caller of a member it is not answered for:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

impl Nested {
    /// The key the holder of `bearer` owns its own members under.
    async fn owner_key_of(&self, bearer: &str) -> TestResult<String> {
        Ok(self
            .ask_as(bearer, crate::rpc::CONSOLE_WHOAMI_METHOD, json!({}))
            .await["result"]["caller_scope"]
            .as_str()
            .ok_or("the caller has a scope")?
            .to_string())
    }

    /// The views `access` gives the owner of the nested mob and the other
    /// person, after one more priming of it from the fixture's runtime.
    async fn primed(&self, access: &AccessController) -> TestResult<(AccessView, AccessView)> {
        prime_access_cache_from_runtime(&self.runtime, access).await;
        Ok((
            access.view_for_caller(
                Some("owner@example.test"),
                Some(&self.owner_key_of(&self.owner).await?),
            ),
            access.view_for_caller(
                Some("other@example.test"),
                Some(&self.owner_key_of(&self.other).await?),
            ),
        ))
    }

    /// Whether the nested member keeps its seat for a second: a retire
    /// that reached a member takes a moment to unseat it, so the roster
    /// is asked every tenth of that second and the first miss answers.
    async fn nested_member_stays_seated(&self) -> TestResult<bool> {
        for _ in 0..10 {
            if !self.nested_member_is_seated().await? {
                return Ok(false);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        self.nested_member_is_seated().await
    }

    /// The nested mob's id.
    fn team_id(&self) -> meerkat_mob::MobId {
        meerkat_mob::MobId::from(self.team.as_str())
    }
}

/// Access rules under which the owner of the fixture's nested mob views
/// every agent but one that carries the label `tier: closed`: a member of
/// which not every label is known (one that has ended) cannot be ruled out
/// of that deny, and is refused.
fn rules_that_refuse_what_is_not_whole() -> AccessController {
    let rule = |id: &str| crate::access::AccessRule {
        id: id.to_string(),
        subjects: vec!["owner@example.test".to_string()],
        actions: vec!["agent.*".to_string()],
        ..crate::access::AccessRule::default()
    };
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![
            crate::access::AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent")
            },
            crate::access::AccessRule {
                effect: crate::access::AccessEffect::Deny,
                match_labels: std::collections::BTreeMap::from([(
                    "tier".to_string(),
                    "closed".to_string(),
                )]),
                ..rule("no-closed-tier")
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("the rules are valid")
}

/// A priming records a member of a nested mob in that mob's own entries,
/// whether or not a member of the launch mob has the one string as its
/// name: the launch mob's member of that spelling is every caller's, and
/// the nested member is its owner's own, known to the view of its mob by
/// its name there and by the one string.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_priming_records_a_nested_member_beside_a_launch_member_of_its_spelling() -> TestResult {
    let nested = Nested::start("member-mob-primed").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let team = nested.team_id();
    let access = AccessController::audience_only();
    let (owner, other) = nested.primed(&access).await?;
    let mut wrong = Vec::new();
    if !other.knows_agent(&one_string)
        || other.hides_agent(&one_string)
        || owner.hides_agent(&one_string)
    {
        wrong
            .push("the launch mob's member under the one string is not every caller's".to_string());
    }
    for name in [NESTED_MEMBER, one_string.as_str()] {
        let (owners, others) = (owner.in_nested_mob(&team), other.in_nested_mob(&team));
        if !owners.knows_agent(name) || owners.hides_agent(name) || !others.hides_agent(name) {
            wrong.push(format!(
                "the nested member, asked of the view of its mob as {name}: known to its owner \
                 {}, hidden from its owner {}, hidden from another person {}",
                owners.knows_agent(name),
                owners.hides_agent(name),
                others.hides_agent(name)
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a priming does not record a nested member beside a launch member of its spelling:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A priming records no agent of the launch mob under a nested member's
/// string: a roster projection of the launch mob, which replaces that
/// mob's agents, leaves the nested member's entry, still its owner's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_priming_records_no_agent_of_the_launch_mob_under_a_nested_members_string() -> TestResult
{
    let nested = Nested::start("member-mob-primed-alone").await?;
    let one_string = nested.nested_by_one_string();
    let access = AccessController::audience_only();
    let (owner, other) = nested.primed(&access).await?;
    if !owner.knows_agent(LAUNCH_MEMBER) || !owner.knows_agent(&one_string) {
        return Err(
            "the fixture: the priming records the launch mob's member and the nested \
                    member"
                .into(),
        );
    }
    assert!(
        !owner.knows_own_agent(&one_string)
            && !owner.hides_agent(&one_string)
            && other.hides_agent(&one_string),
        "right after the priming: the launch mob holds an agent under the nested member's \
         string {}, hidden from its owner {}, hidden from another person {}",
        owner.knows_own_agent(&one_string),
        owner.hides_agent(&one_string),
        other.hides_agent(&one_string)
    );
    access.replace_agent_attributes([AgentResourceAttributes {
        identity: "another".to_string(),
        agent_id: None,
        role: None,
        labels: std::collections::BTreeMap::new(),
    }]);
    if owner.knows_agent(LAUNCH_MEMBER) {
        return Err("the fixture: the projection replaced the launch mob's agents".into());
    }
    assert!(
        owner.knows_agent(&one_string)
            && !owner.hides_agent(&one_string)
            && other.hides_agent(&one_string),
        "the nested member's entry was among the launch mob's agents: known {}, hidden from \
         its owner {}, hidden from another person {}",
        owner.knows_agent(&one_string),
        owner.hides_agent(&one_string),
        other.hides_agent(&one_string)
    );
    Ok(())
}

/// A nested member that ended keeps an entry in its mob's own entries,
/// also where a live member of the launch mob has the one string as its
/// name: it is known there, still its owner's own, and no longer viewed
/// as a whole member under a rule that what is known of it cannot rule
/// out. The launch mob's member of that spelling is decided by its own
/// entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_priming_records_the_end_of_a_nested_member_in_its_mobs_own_entries() -> TestResult {
    let nested = Nested::start("member-mob-primed-ended").await?;
    let one_string = nested.nested_by_one_string();
    let team = nested.team_id();
    let access = rules_that_refuse_what_is_not_whole();
    let (owner, _) = nested.primed(&access).await?;
    if !owner.can_view_agent(&one_string) {
        return Err(
            "the fixture: the live nested member is viewed by its owner under the rules".into(),
        );
    }
    let retired = nested
        .ask(
            "meta/retire",
            json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team }),
        )
        .await;
    if !retired["error"].is_null() || !nested.nested_member_is_retired().await {
        return Err(format!("the fixture: the nested member is retired: {retired}").into());
    }
    nested.seat_the_namesake().await?;
    let (owner, other) = nested.primed(&access).await?;
    let mut wrong = Vec::new();
    for name in [NESTED_MEMBER, one_string.as_str()] {
        let (owners, others) = (owner.in_nested_mob(&team), other.in_nested_mob(&team));
        if !owners.knows_agent(name)
            || owners.hides_agent(name)
            || !others.hides_agent(name)
            || owners.can_view_agent(name)
        {
            wrong.push(format!(
                "the ended nested member, asked of the view of its mob as {name}: known {}, \
                 hidden from its owner {}, hidden from another person {}, viewed by its owner {}",
                owners.knows_agent(name),
                owners.hides_agent(name),
                others.hides_agent(name),
                owners.can_view_agent(name)
            ));
        }
    }
    if other.hides_agent(&one_string) || !owner.can_view_agent(&one_string) {
        wrong.push(
            "the live member of the launch mob under the one string is not decided by its own \
             entry"
                .to_string(),
        );
    }
    assert!(
        wrong.is_empty(),
        "a priming does not record the end of a nested member in its mob's own entries:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// A nested mob that is destroyed whole leaves no member of it viewed as a
/// whole member: after the next priming its member is refused under a rule
/// that what is known of it cannot rule out, in the view of its mob and in
/// the view of the launch mob alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_priming_after_a_nested_mob_is_destroyed_reads_none_of_its_members_as_whole() -> TestResult
{
    let nested = Nested::start("member-mob-primed-destroyed").await?;
    let one_string = nested.nested_by_one_string();
    let team = nested.team_id();
    let access = rules_that_refuse_what_is_not_whole();
    let (owner, _) = nested.primed(&access).await?;
    if !owner.can_view_agent(&one_string) {
        return Err(
            "the fixture: the live nested member is viewed by its owner under the rules".into(),
        );
    }
    let mobs = nested
        .runtime
        .agent_mob_mcp_state()
        .ok_or("the runtime carries agent mob tools")?;
    Box::pin(mobs.mob_destroy(&team))
        .await
        .map_err(|error| format!("the fixture: the nested mob is destroyed: {error:?}"))?;
    let (owner, _) = nested.primed(&access).await?;
    assert!(
        !owner.in_nested_mob(&team).can_view_agent(NESTED_MEMBER)
            && !owner.can_view_agent(&one_string),
        "a member of the destroyed mob is still viewed as a whole member: in the view of its \
         mob {}, in the view of the launch mob {}",
        owner.in_nested_mob(&team).can_view_agent(NESTED_MEMBER),
        owner.can_view_agent(&one_string)
    );
    // On a console without rules the member stays its owner's own: the
    // record that outlives the mob says who it was private to.
    let audience = AccessController::audience_only();
    let (_, other) = nested.primed(&audience).await?;
    assert!(
        other.hides_agent(&one_string),
        "a member of the destroyed mob is no longer hidden from another person"
    );
    Ok(())
}

/// After a priming, the member of a nested mob that was destroyed whole
/// keeps an entry in that mob's own entries: it is known there, and hidden
/// from another person there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_priming_after_a_nested_mob_is_destroyed_keeps_its_members_end_in_its_own_entries()
-> TestResult {
    let nested = Nested::start("member-mob-primed-destroyed-entry").await?;
    let team = nested.team_id();
    let mobs = nested
        .runtime
        .agent_mob_mcp_state()
        .ok_or("the runtime carries agent mob tools")?;
    Box::pin(mobs.mob_destroy(&team))
        .await
        .map_err(|error| format!("the fixture: the nested mob is destroyed: {error:?}"))?;
    let access = AccessController::audience_only();
    let (owner, other) = nested.primed(&access).await?;
    let (owners, others) = (owner.in_nested_mob(&team), other.in_nested_mob(&team));
    assert!(
        owners.knows_agent(NESTED_MEMBER)
            && !owners.hides_agent(NESTED_MEMBER)
            && others.hides_agent(NESTED_MEMBER),
        "the member of the destroyed mob in its mob's own entries: known {}, hidden from its \
         owner {}, hidden from another person {}",
        owners.knows_agent(NESTED_MEMBER),
        owners.hides_agent(NESTED_MEMBER),
        others.hides_agent(NESTED_MEMBER)
    );
    Ok(())
}

impl Nested {
    /// The answer of a console route for the holder of `bearer`: its status
    /// and its JSON body (null for a body that is no JSON).
    async fn route_as(
        &self,
        bearer: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        use tower::ServiceExt;
        let request = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {bearer}"));
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body.to_string())),
            None => request.body(axum::body::Body::empty()),
        }
        .expect("route request");
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("route response");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 22)
            .await
            .expect("route body");
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }
}

/// Whether a route's answer is its refusal of `member_mob_id`: a bad
/// request with the kind the request door refuses the field with on a
/// method that does not take it.
fn is_the_routes_refusal_of_the_field(status: StatusCode, answer: &Value) -> bool {
    status == StatusCode::BAD_REQUEST && answer["error"] == json!("member_mob_id_not_accepted")
}

/// The send route takes no `member_mob_id`, and says so: a send that
/// carries the field is refused with `member_mob_id_not_accepted`, the kind
/// the request door refuses the field with on a method that does not take
/// it. The refusal is read from the body's shape alone: it is the same
/// whatever the send names and whoever sends it, and no member takes the
/// message. A `null` field names no mob, as on the request door, and the
/// send is taken as before.
///
/// The other doors of a member: on the request door `meta/console/send`
/// and `meta/console/query_timeline` take the field and
/// `meta/console/recover_send` refuses it with that kind
/// (`every_method_of_one_member_takes_member_mob_id_or_refuses_it`); the
/// recovery route and the timeline routes are the next two rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_send_route_refuses_member_mob_id() -> TestResult {
    let nested = Nested::start("member-mob-route-field").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let launch_session = nested
        .session_of(&nested.other, json!({ "identity": one_string }))
        .await;
    let send = |identity: &str, key: &str, member_mob: Value| {
        json!({
            "identity": identity,
            "content": content_of(key),
            "origin": "console",
            "idempotency_key": key,
            "member_mob_id": member_mob,
        })
    };
    let mut wrong = Vec::new();
    let mut refusals = Vec::new();
    for (who, bearer, identity, key) in [
        (
            "the owner, the nested member's name",
            &nested.owner,
            NESTED_MEMBER,
            "field-1",
        ),
        (
            "the owner, the one string",
            &nested.owner,
            one_string.as_str(),
            "field-2",
        ),
        (
            "the other person, the one string",
            &nested.other,
            one_string.as_str(),
            "field-3",
        ),
        (
            "the owner, a name no member holds",
            &nested.owner,
            "no-such-member",
            "field-4",
        ),
    ] {
        let (status, answer) = nested
            .route_as(
                bearer,
                "POST",
                "/console/send",
                Some(send(identity, key, json!(nested.team))),
            )
            .await;
        if !is_the_routes_refusal_of_the_field(status, &answer) {
            wrong.push(format!("{who}: {status} {answer}"));
        }
        refusals.push(answer);
    }
    if refusals.windows(2).any(|pair| pair[0] != pair[1]) {
        wrong.push(format!("the refusals differ: {refusals:?}"));
    }
    for key in ["field-2", "field-3"] {
        if nested
            .page_holds(&nested.other, json!({ "identity": one_string }), key)
            .await
        {
            wrong.push(format!(
                "the launch mob's member took the message sent under {key}"
            ));
        }
    }
    for key in ["field-1", "field-2", "field-3", "field-4"] {
        if nested.nested_member_took(key).await {
            wrong.push(format!(
                "the nested member took the message sent under {key}"
            ));
        }
    }
    let (status, sent) = nested
        .route_as(
            &nested.owner,
            "POST",
            "/console/send",
            Some(send(&one_string, "field-null", Value::Null)),
        )
        .await;
    if status != StatusCode::OK || launch_session.is_null() || sent["session_id"] != launch_session
    {
        wrong.push(format!(
            "a send with a null field is not taken on the session {launch_session} of the launch \
             mob's member: {status} {sent}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "the send route does not refuse member_mob_id as the request door does:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The recovery route takes no `member_mob_id` and says so as the send
/// route does: the same kind, read from the body's shape alone, the same
/// whatever the recovery names and whoever asks. A `null` field names no
/// mob: the recovery is answered as the recovery without the field.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_recovery_route_refuses_member_mob_id() -> TestResult {
    let nested = Nested::start("member-mob-recover-field").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let recovery = |identity: &str, member_mob: Option<Value>| {
        let mut body = json!({
            "identity": identity,
            "origin": "console",
            "idempotency_key": "never-sent",
        });
        if let Some(member_mob) = member_mob {
            body["member_mob_id"] = member_mob;
        }
        body
    };
    let mut wrong = Vec::new();
    let mut refusals = Vec::new();
    for (who, bearer, identity) in [
        (
            "the owner, the one string",
            &nested.owner,
            one_string.as_str(),
        ),
        (
            "the other person, the one string",
            &nested.other,
            one_string.as_str(),
        ),
        (
            "the owner, a name no member holds",
            &nested.owner,
            "no-such-member",
        ),
    ] {
        let (status, answer) = nested
            .route_as(
                bearer,
                "POST",
                "/console/send/recover",
                Some(recovery(identity, Some(json!(nested.team)))),
            )
            .await;
        if !is_the_routes_refusal_of_the_field(status, &answer) {
            wrong.push(format!("{who}: {status} {answer}"));
        }
        refusals.push(answer);
    }
    if refusals.windows(2).any(|pair| pair[0] != pair[1]) {
        wrong.push(format!("the refusals differ: {refusals:?}"));
    }
    let without = nested
        .route_as(
            &nested.owner,
            "POST",
            "/console/send/recover",
            Some(recovery(&one_string, None)),
        )
        .await;
    let with_null = nested
        .route_as(
            &nested.owner,
            "POST",
            "/console/send/recover",
            Some(recovery(&one_string, Some(Value::Null))),
        )
        .await;
    if with_null != without || is_the_routes_refusal_of_the_field(without.0, &without.1) {
        wrong.push(format!(
            "a recovery with a null field is not answered as the recovery without the field: \
             {with_null:?}; without: {without:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "the recovery route does not refuse member_mob_id as the request door does:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The timeline route and the timeline streams (the stream of one
/// identity too) take no `member_mob_id` and say so as the send route
/// does: the same kind, whatever the read names and whoever asks, and no
/// stream begins. A read without the field is answered the page of the
/// launch mob's member the string names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_timeline_routes_refuse_member_mob_id() -> TestResult {
    let nested = Nested::start("member-mob-timeline-field").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let sent = nested.send_by_the_route_as(&nested.other, "seen").await;
    assert!(
        sent.is_ok(),
        "the fixture: the other person's send by the route is not taken: {sent:?}"
    );
    let mut wrong = Vec::new();
    let plain = format!("/console/timeline?identity={one_string}");
    let (status, page) = nested.route_as(&nested.other, "GET", &plain, None).await;
    if status != StatusCode::OK || !page.to_string().contains(&content_of("seen")) {
        wrong.push(format!("the read without the field: {status} {page}"));
    }
    let stream_of_one = format!("/console/identity/{LAUNCH_MEMBER}/stream");
    for route in [
        "/console/timeline",
        "/console/timeline/stream",
        stream_of_one.as_str(),
    ] {
        for named in [format!("identity={one_string}&"), String::new()] {
            let uri = format!("{route}?{named}member_mob_id={}", nested.team);
            for (who, bearer) in [
                ("the owner", &nested.owner),
                ("the other person", &nested.other),
            ] {
                let answered = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    nested.route_as(bearer, "GET", &uri, None),
                )
                .await;
                match answered {
                    Ok((status, answer)) if is_the_routes_refusal_of_the_field(status, &answer) => {
                    }
                    Ok((status, answer)) => wrong.push(format!("{who}, {uri}: {status} {answer}")),
                    Err(_) => wrong.push(format!(
                        "{who}, {uri}: no answer within ten seconds: a stream began"
                    )),
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "the timeline routes do not refuse member_mob_id as the request door does:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

impl Nested {
    /// The fixture of the rows of an ended launch member that every caller
    /// was answered for: it held the one string, took one message of the
    /// other person and ended, before the nested mob existed.
    async fn with_an_ended_namesake(tag: &str) -> TestResult<Self> {
        let nested = Self::start_without_the_nested_mob(tag).await?;
        nested.seat_the_namesake().await?;
        nested
            .send_by_the_route_as(&nested.other, "to-the-ended")
            .await
            .map_err(|refused| {
                format!("the fixture: the launch mob's member takes no send: {refused}")
            })?;
        nested.end_the_namesake().await?;
        Ok(nested)
    }

    /// The nested mob is created and its member takes one message of its
    /// owner, named by its two values.
    async fn then_a_live_nested_member(&self) -> TestResult {
        self.create_the_nested_mob().await?;
        let sent = self
            .send_as(
                &self.owner,
                json!({ "identity": NESTED_MEMBER, "member_mob_id": self.team }),
                "to-the-live",
            )
            .await;
        if !sent["error"].is_null() {
            return Err(format!("the fixture: the nested member takes no send: {sent}").into());
        }
        Ok(())
    }

    /// Whether the nested member took the message sent under `key`, as its
    /// owner reads its page by its two values.
    async fn nested_member_took(&self, key: &str) -> bool {
        self.page_holds(
            &self.owner,
            json!({ "identity": NESTED_MEMBER, "member_mob_id": self.team }),
            key,
        )
        .await
    }
}

/// The methods that retire the member one string names, each with the
/// field it reads its member from.
const RETIRES_BY_ONE_STRING: &[(&str, &str)] = &[
    ("meta/retire", "identity"),
    ("meta/retire_member", "member_id"),
];

/// An ended member of the launch mob that every caller was answered for
/// opens no way to a live member of a nested mob of its spelling that is
/// another person's own. The other person asks the request door by the one
/// string before the nested mob exists and after: each read of one member
/// is answered as before, the timeline holds the same frames, a send is
/// answered as before and the nested member takes no message, and each
/// retire is answered as before and the nested member keeps its seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_launch_member_opens_no_way_to_a_nested_member_of_its_spelling() -> TestResult {
    let nested = Nested::with_an_ended_namesake("member-mob-ended-way").await?;
    let one_string = nested.nested_by_one_string();
    let reads = READS_OF_ONE_MEMBER
        .iter()
        .filter(|(method, _)| *method != "meta/console/query_timeline");
    let by_string = json!({ "identity": one_string });
    let mut before = Vec::new();
    for (method, field) in reads.clone() {
        before.push(
            nested
                .ask_as(&nested.other, method, json!({ *field: one_string }))
                .await,
        );
    }
    let page_before = frame_ids(
        &nested
            .ask_as(
                &nested.other,
                "meta/console/query_timeline",
                by_string.clone(),
            )
            .await,
    );
    let send_before = nested
        .send_as(&nested.other, by_string.clone(), "before")
        .await;
    let mut retires_before = Vec::new();
    for (method, field) in RETIRES_BY_ONE_STRING {
        retires_before.push(
            nested
                .ask_as(&nested.other, method, json!({ *field: one_string }))
                .await,
        );
    }
    nested.then_a_live_nested_member().await?;
    let mut wrong = Vec::new();
    for ((method, field), before) in reads.zip(before) {
        let after = nested
            .ask_as(&nested.other, method, json!({ *field: one_string }))
            .await;
        if apart_from_progress(&after) != apart_from_progress(&before) {
            wrong.push(format!(
                "{method}: once the nested member lives: {after}\n  before: {before}"
            ));
        }
    }
    let page_after = frame_ids(
        &nested
            .ask_as(
                &nested.other,
                "meta/console/query_timeline",
                by_string.clone(),
            )
            .await,
    );
    if page_before.is_empty() || page_after != page_before {
        wrong.push(format!(
            "the timeline: once the nested member lives: {page_after:?}\n  before: {page_before:?}"
        ));
    }
    let send_after = nested.send_as(&nested.other, by_string, "after").await;
    if send_before["result"].is_null() != send_after["result"].is_null()
        || send_before["error"]["code"] != send_after["error"]["code"]
        || nested.nested_member_took("after").await
    {
        wrong.push(format!(
            "the send: once the nested member lives: {send_after}\n  before: {send_before}"
        ));
    }
    for ((method, field), before) in RETIRES_BY_ONE_STRING.iter().zip(retires_before) {
        let after = nested
            .ask_as(&nested.other, method, json!({ *field: one_string }))
            .await;
        if after != before || !nested.nested_member_stays_seated().await? {
            wrong.push(format!(
                "{method}: once the nested member lives: {after}\n  before: {before}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "an ended launch member opens a way to a nested member of its spelling:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// [`an_ended_launch_member_opens_no_way_to_a_nested_member_of_its_spelling`]
/// for the send route: the other person's send by the one string is
/// answered as before the nested mob existed, and the nested member takes
/// no message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_launch_member_opens_no_way_to_a_nested_member_through_the_send_route()
-> TestResult {
    let nested = Nested::with_an_ended_namesake("member-mob-ended-route").await?;
    let before = nested.send_by_the_route_as(&nested.other, "before").await;
    nested.then_a_live_nested_member().await?;
    let after = nested.send_by_the_route_as(&nested.other, "after").await;
    let as_before = match (&before, &after) {
        (Err(before), Err(after)) => before == after,
        _ => false,
    };
    assert!(
        as_before && !nested.nested_member_took("after").await,
        "an ended launch member opens a way to a nested member of its spelling through the \
         send route: {after:?}\n  before: {before:?}"
    );
    Ok(())
}

/// A member is answered for only where the caller's view of its mob knows
/// it. Another person's own member of the launch mob holds the one string
/// of the owner's nested member. With the records read, the owner is
/// answered for its nested member and not for that other member. Once the
/// launch mob's agents in the records are replaced by a roster that does
/// not hold that member, the view of the launch mob does not know it, and
/// asked about the string it reads the nested member, which it does not
/// hide from its owner: the owner is still not answered for the launch
/// mob's member. The row brings the records into that state by replacing
/// the launch mob's agents; a console is in it for a member seated since
/// its records were last read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_its_mobs_view_does_not_know_is_answered_for_to_no_caller() -> TestResult {
    let nested = Nested::start_without_the_nested_mob("member-mob-unknown").await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let one_string = nested.nested_by_one_string();
    let team = nested.team_id();
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        nested.runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );
    let access = AccessController::audience_only();
    let (owner, _) = nested.primed(&access).await?;
    let method = "meta/console/inspect_identity";
    // The console's one mob is its host's default mob: a stored record
    // names it by no mob.
    let launch = None;
    let own =
        caller_is_answered_for(&aggregator, launch, Some(&owner), method, None, &one_string).await;
    let of_the_nested_mob = caller_is_answered_for(
        &aggregator,
        launch,
        Some(&owner),
        method,
        Some(&team),
        &one_string,
    )
    .await;
    if own || !of_the_nested_mob {
        return Err(format!(
            "the fixture: with the records read the owner is answered for the launch mob's \
             member {own}, for its nested member {of_the_nested_mob}"
        )
        .into());
    }
    access.replace_agent_attributes([AgentResourceAttributes {
        identity: "another".to_string(),
        agent_id: None,
        role: None,
        labels: std::collections::BTreeMap::new(),
    }]);
    if owner.knows_own_agent(&one_string) || owner.hides_agent(&one_string) {
        return Err(
            "the fixture: the view of the launch mob still knows the member, or hides the string"
                .into(),
        );
    }
    assert!(
        !caller_is_answered_for(&aggregator, launch, Some(&owner), method, None, &one_string).await,
        "the owner is answered for a member of the launch mob that its view does not know"
    );
    Ok(())
}

/// A send by one string is recovered by it. Another person's own member of
/// the launch mob holds the one string of the owner's nested member. The
/// owner's send by the string on the request door is taken by the nested
/// member, and the recovery of that send by the same string finds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_by_one_string_to_a_nested_member_is_recovered_by_the_string() -> TestResult {
    let nested = Nested::start_without_the_nested_mob("member-mob-recover-one").await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let one_string = nested.nested_by_one_string();
    let sent = nested
        .send_as(
            &nested.owner,
            json!({ "identity": one_string }),
            "to-recover",
        )
        .await;
    let recovered = nested
        .ask(
            "meta/console/recover_send",
            json!({
                "identity": one_string,
                "origin": "console",
                "idempotency_key": "to-recover",
            }),
        )
        .await;
    assert!(
        sent["error"].is_null()
            && !sent["result"].is_null()
            && recovered["error"].is_null()
            && !recovered["result"].is_null(),
        "the owner's send by the one string: {sent}\n  its recovery by the string: {recovered}"
    );
    Ok(())
}

/// [`a_send_by_one_string_to_a_nested_member_is_recovered_by_the_string`]
/// through the send route and the recovery route.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_by_the_route_to_a_nested_member_is_recovered_by_the_route() -> TestResult {
    let nested = Nested::start_without_the_nested_mob("member-mob-recover-route").await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let sent = nested
        .send_by_the_route_as(&nested.owner, "to-recover")
        .await;
    let (status, recovered) = nested
        .route_as(
            &nested.owner,
            "POST",
            "/console/send/recover",
            Some(json!({
                "identity": nested.nested_by_one_string(),
                "origin": "console",
                "idempotency_key": "to-recover",
            })),
        )
        .await;
    assert!(
        sent.is_ok() && status == StatusCode::OK,
        "the owner's send by the route: {sent:?}\n  its recovery by the route: {status} {recovered}"
    );
    Ok(())
}

/// THE ROSTER AND A PAIR. Where a member of the launch mob and a member of
/// a nested mob hold one string, the roster of live members shows neither:
/// it keeps a member's response phase and last activity by the string, so
/// a row for one would show the other's. The listing of identities holds
/// both, and each is reached by the mob it sits in and its name there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_roster_shows_neither_member_of_one_spelling() -> TestResult {
    let nested = Nested::start("member-mob-roster-pair").await?;
    nested.seat_the_namesake().await?;
    let one_string = nested.nested_by_one_string();
    let launch_member = json!({ "identity": one_string, "member_mob_id": nested.launch_mob });
    let nested_member = json!({ "identity": NESTED_MEMBER, "member_mob_id": nested.team });
    let mut wrong = Vec::new();
    // Each takes a message, so each has a phase and an activity to show.
    for (who, member, key) in [
        ("the launch mob's member", &launch_member, "to-the-launch"),
        ("the nested member", &nested_member, "to-the-nested"),
    ] {
        let sent = nested.send_as(&nested.owner, member.clone(), key).await;
        if !sent["error"].is_null() {
            wrong.push(format!(
                "the fixture: {who} takes no send by its two values: {sent}"
            ));
        }
    }
    let launch_session = nested.session_of(&nested.owner, launch_member).await;
    let nested_session = nested.session_of(&nested.owner, nested_member).await;
    if launch_session.is_null() || nested_session.is_null() || launch_session == nested_session {
        wrong.push(format!(
            "the two members are not each reached by their two values: the launch mob's member \
             on {launch_session}, the nested member on {nested_session}"
        ));
    }
    let listed = nested.ask("meta/console/list_identities", json!({})).await;
    let holders = listed
        .pointer("/result/identities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|record| record["identity"] == json!(one_string))
        .count();
    if holders != 2 {
        wrong.push(format!(
            "the listing of identities holds {holders} members under the string: {listed}"
        ));
    }
    for (who, bearer) in [
        ("the owner", &nested.owner),
        ("the other person", &nested.other),
    ] {
        if let Some(seen) = roster_holds_the_string(&nested.app, bearer, &one_string).await {
            wrong.push(format!(
                "{who}'s roster holds the string, or does not hold the launch mob's first \
                 member: {seen}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the roster and two members of one spelling:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// [`an_ended_launch_member_opens_no_way_to_a_nested_member_of_its_spelling`]
/// asked by the roster id of the launch mob that spells the one string. The
/// console's doors take no roster id: the listing would decode it to the
/// string, and the doors refuse the reserved spelling before anything is
/// resolved. So the other person's reads, sends and retires by it are
/// answered the same before the nested mob exists and after, on the request
/// door and through the send route, and the nested member takes no message
/// and keeps its seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_launch_member_opens_no_way_by_its_roster_id() -> TestResult {
    let nested = Nested::with_an_ended_namesake("member-mob-ended-roster-id").await?;
    let one_string = nested.nested_by_one_string();
    let roster_id = crate::member_comms_id::mob_member_id_str(&one_string).into_owned();
    if roster_id == one_string {
        return Err("the fixture: the roster id is the string itself".into());
    }
    let asks = [
        (
            "meta/console/inspect_identity",
            json!({ "identity": roster_id }),
        ),
        ("meta/inspect_identity", json!({ "identity": roster_id })),
        ("meta/status_identity", json!({ "identity": roster_id })),
        ("meta/retire", json!({ "identity": roster_id })),
        ("meta/retire_member", json!({ "member_id": roster_id })),
    ];
    let route_send = |key: &str| {
        json!({
            "identity": roster_id,
            "content": content_of(key),
            "origin": "console",
            "idempotency_key": key,
        })
    };
    let mut before = Vec::new();
    for (method, params) in &asks {
        before.push(nested.ask_as(&nested.other, method, params.clone()).await);
    }
    let send_before = nested
        .send_as(&nested.other, json!({ "identity": roster_id }), "before")
        .await;
    let route_before = nested
        .route_as(
            &nested.other,
            "POST",
            "/console/send",
            Some(route_send("route-before")),
        )
        .await;
    nested.then_a_live_nested_member().await?;
    let mut wrong = Vec::new();
    let send_after = nested
        .send_as(&nested.other, json!({ "identity": roster_id }), "after")
        .await;
    if send_before["result"].is_null() != send_after["result"].is_null()
        || send_before["error"]["code"] != send_after["error"]["code"]
        || nested.nested_member_took("after").await
    {
        wrong.push(format!(
            "the send: once the nested member lives: {send_after}\n  before: {send_before}"
        ));
    }
    let route_after = nested
        .route_as(
            &nested.other,
            "POST",
            "/console/send",
            Some(route_send("route-after")),
        )
        .await;
    if route_after.0 != route_before.0
        || route_after.1["error"] != route_before.1["error"]
        || nested.nested_member_took("route-after").await
    {
        wrong.push(format!(
            "the send route: once the nested member lives: {route_after:?}\n  before: \
             {route_before:?}"
        ));
    }
    for ((method, params), before) in asks.iter().zip(before) {
        let after = nested.ask_as(&nested.other, method, params.clone()).await;
        if apart_from_progress(&after) != apart_from_progress(&before)
            || !nested.nested_member_stays_seated().await?
        {
            wrong.push(format!(
                "{method}: once the nested member lives: {after}\n  before: {before}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "an ended launch member opens a way to a nested member of its spelling by its roster \
         id:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// What is wrong with the roster of live members the holder of `bearer`
/// reads on `app`, where two members hold `one_string`: none when the
/// roster holds the row of the launch mob's first member and holds the
/// string nowhere.
async fn roster_holds_the_string(app: &Router, bearer: &str, one_string: &str) -> Option<String> {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/console/experience")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::empty())
                .expect("experience request"),
        )
        .await
        .expect("experience response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("experience body");
    let seen: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let roster = seen
        .pointer("/agent_sidebar")
        .map(Value::to_string)
        .unwrap_or_default();
    let first_member_listed = seen
        .pointer("/agent_sidebar/live_snapshot/agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|row| row["agent_id"] == json!(LAUNCH_MEMBER));
    (status != StatusCode::OK || !first_member_listed || roster.contains(one_string))
        .then(|| format!("{status} {roster}"))
}

/// [`the_roster_shows_neither_member_of_one_spelling`] where the launch
/// mob's member of the pair is another person's own: the roster of that
/// person and the roster of the nested member's owner show neither, and
/// each person's listing of identities holds its own member under the
/// string.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_roster_shows_neither_member_of_one_spelling_where_one_is_another_persons_own()
-> TestResult {
    // The other person's member takes the string first: a fork under the
    // string of another person's nested member is answered an unknown
    // identity.
    let nested = Nested::start_without_the_nested_mob("member-mob-roster-own").await?;
    nested.fork_the_namesake_as(&nested.other).await?;
    nested.create_the_nested_mob().await?;
    let one_string = nested.nested_by_one_string();
    let mut wrong = Vec::new();
    // The owner's own member under the string is the nested member; the
    // other person's is the launch mob's.
    for (who, bearer, its_own_is_nested) in [
        ("the owner", &nested.owner, true),
        ("the other person", &nested.other, false),
    ] {
        if let Some(seen) = roster_holds_the_string(&nested.app, bearer, &one_string).await {
            wrong.push(format!(
                "{who}'s roster holds the string, or not the launch mob's first member: {seen}"
            ));
        }
        let listed = nested
            .ask_as(bearer, "meta/console/list_identities", json!({}))
            .await;
        // Each member listed under the string, by whether it is the nested
        // mob's.
        let of_the_nested_mob: Vec<bool> = listed
            .pointer("/result/identities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|record| record["identity"] == json!(one_string))
            .map(|record| record["labels"]["source_mob_id"] == json!(nested.team))
            .collect();
        if of_the_nested_mob.contains(&!its_own_is_nested) {
            wrong.push(format!(
                "{who}'s listing of identities holds the other person's member under the \
                 string: {listed}"
            ));
        }
        if of_the_nested_mob != [its_own_is_nested] {
            wrong.push(format!(
                "{who}'s listing of identities does not hold exactly its own member under the \
                 string: {listed}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the roster and two members of one spelling, one of them another person's own:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

/// The listing of identities decides a record in the view of a nested mob
/// only where the access records keep that mob's members apart: a record
/// of a runtime the console names without a namespace, whose label names
/// the mob and whose string has the form of that mob's member. Under a
/// namespace the record is decided in the caller's own view, where its
/// member's entry is recorded: also where the namespace is the nested
/// mob's id, so that the record's string has the form of that mob's
/// member.
#[test]
fn the_listing_decides_a_record_in_a_nested_mobs_view_only_without_a_namespace() {
    let of_the_team =
        std::collections::BTreeMap::from([("source_mob_id".to_string(), "team".to_string())]);
    let no_label = std::collections::BTreeMap::new();
    let mut wrong = Vec::new();
    for (what, identity, labels, namespace, expected) in [
        (
            "a nested member's record, no namespace",
            "team/helper",
            &of_the_team,
            "",
            Some("team"),
        ),
        (
            "a nested member's record, a namespace of slashes and blanks",
            "team/helper",
            &of_the_team,
            " / ",
            Some("team"),
        ),
        (
            "a nested member's record under a namespace that is the mob's id",
            "team/helper",
            &of_the_team,
            "team",
            None,
        ),
        (
            "a nested member's record under another namespace",
            "desk/team/helper",
            &of_the_team,
            "desk",
            None,
        ),
        (
            "a record with the mob's form and no label of a mob",
            "team/helper",
            &no_label,
            "",
            None,
        ),
        (
            "a record with the label and another form",
            "helper",
            &of_the_team,
            "",
            None,
        ),
    ] {
        let read = super::listed_records_nested_mob(identity, labels, namespace);
        if read != expected {
            wrong.push(format!("{what}: read {read:?}, not {expected:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "the nested mob in whose view the listing decides a record:\n{}",
        wrong.join("\n")
    );
}
