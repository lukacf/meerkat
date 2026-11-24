//! Rows for the console's lists that are the host's: one list for a person,
//! whatever mob a request names, each entry judged in the entry's own mob.

#![allow(clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use super::mobs_selector_tests::{ADMIN, Fixture, TwoMobs, WITHOUT_RULES, rpc};
use super::*;

/// The gating methods the console serves.
const GATING_METHODS: &[&str] = &[
    "meta/gating/audit",
    "meta/gating/decide",
    "meta/gating/pending",
];

/// The gating state is the host's, one for all its mobs: a gating request
/// reads no value of any mob, so the console's door resolves no mob for
/// it, whatever mob it names (resolving is what loads a mob that is known
/// and not loaded). The id the request names is still judged with its
/// caller and asked of the host: a caller who may address the mob is
/// answered the host's gating method, and a caller who may not is refused
/// like an id no mob has.
#[tokio::test]
async fn a_gating_request_that_names_another_mob_resolves_no_mob() {
    let console = TwoMobs::console().await;
    let mut wrong = Vec::new();
    for method in GATING_METHODS {
        let answer = console
            .ask(
                rpc(method, json!({ "mob_id": console.second_mob })),
                Some(ADMIN),
            )
            .await;
        let body: Value = serde_json::from_str(&answer.1).unwrap_or(Value::Null);
        // The fixture console has no gating owner: the host's method
        // answers that it is not served, after the door let the request in.
        if answer.0 != StatusCode::OK || body["error"]["code"] != json!(-32601) {
            wrong.push(format!(
                "{method} as an administrator: not the host's answer: {answer:?}"
            ));
        }
        let refused = console
            .ask(
                rpc(method, json!({ "mob_id": console.second_mob })),
                Some(WITHOUT_RULES),
            )
            .await;
        let body: Value = serde_json::from_str(&refused.1).unwrap_or(Value::Null);
        if body["error"]["data"]["kind"] != json!("foreign_mob_target") {
            wrong.push(format!(
                "{method} as a person no rule names: not refused like an id no mob has: \
                 {refused:?}"
            ));
        }
    }
    let resolved = console.resolver.resolved_for_in_tests();
    if resolved.iter().any(Option::is_some) {
        wrong.push(format!(
            "a gating request resolved the mob it named: {resolved:?}"
        ));
    }
    let asked = console.resolver.asked_whether_served_in_tests();
    if !asked
        .iter()
        .flatten()
        .any(|mob| mob.as_str() == console.second_mob)
    {
        wrong.push(format!(
            "the id a gating request named was not asked of the host: {asked:?}"
        ));
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a gating request is not served as a method of the host:\n{}",
        wrong.join("\n")
    );
}

/// A console whose state holds the launch runtime's gating owner: the
/// host's one gating state, for the entries of every mob's members.
async fn console_with_gating() -> TwoMobs {
    TwoMobs::console_with(Fixture {
        gating: true,
        audit: true,
        ..Fixture::default()
    })
    .await
}

/// A pending gating entry whose action came from the member `alice` of the
/// mob `mob` (none: an origin that names no mob), recorded in the host's
/// gating state. Its pending id.
async fn pending_entry_of(console: &TwoMobs, mob: Option<&str>) -> String {
    let request = json!({
        "action": "Publish the reviewed report",
        "actor_id": "agent:writer",
        "risk_tier": "r3",
        "approval_timeout_ms": 600_000,
    });
    console
        .launch
        .module_runtime_handle()
        .lock()
        .await
        .evaluate_gating_action_with_origin(
            serde_json::from_value(request).expect("gating request"),
            Some(crate::runtime::GatingOrigin {
                mob_id: mob.map(meerkat_mob::MobId::from),
                identity: "alice".to_string(),
                conversation_id: None,
                interaction_id: None,
            }),
        )
        .pending_id
        .expect("an R3 gate waits for a decision")
}

/// The answer of `method` with `params` to an administrator, as JSON.
async fn asked(console: &TwoMobs, method: &str, params: Value) -> Value {
    let answer = console.ask(rpc(method, params), Some(ADMIN)).await;
    serde_json::from_str(&answer.1).unwrap_or_else(|_| panic!("{method}: no JSON: {answer:?}"))
}

/// The pending ids the entries at `pointer` of `answer` name, sorted.
fn pending_ids(answer: &Value, pointer: &str) -> Vec<String> {
    let mut ids: Vec<String> = answer
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no list at {pointer}: {answer}"))
        .iter()
        .filter_map(|entry| entry["pending_id"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The gating lists as an administrator reads them with `params`: the
/// pending ids of the pending list and of the audit read.
async fn gating_lists(console: &TwoMobs, params: &Value) -> (Vec<String>, Vec<String>) {
    let pending = asked(console, "meta/gating/pending", params.clone()).await;
    let audit = asked(console, "meta/gating/audit", params.clone()).await;
    (
        pending_ids(&pending, "/result/pending"),
        pending_ids(&audit, "/result/entries"),
    )
}

/// A request that does not select every mob is answered, of the entries
/// members made, the launch mob's members' of the host's gating lists and
/// no entry of another mob's member, even to a caller who may see both: the
/// list a console of one mob was answered.
/// An entry whose origin names the launch mob by its id is the launch
/// mob's. A mob named beside the method selects nothing.
#[tokio::test]
async fn without_a_mobs_selector_a_gating_list_answers_the_launch_mobs_entries_alone() {
    let console = console_with_gating().await;
    let of_launch = pending_entry_of(&console, None).await;
    let of_launch_by_id = pending_entry_of(&console, Some(&console.launch_mob)).await;
    let of_second = pending_entry_of(&console, Some(&console.second_mob)).await;
    let mut of_the_launch_mob = vec![of_launch, of_launch_by_id];
    of_the_launch_mob.sort();

    let mut wrong = Vec::new();
    for params in [json!({}), json!({ "mob_id": console.second_mob })] {
        let (pending, audit) = gating_lists(&console, &params).await;
        if pending != of_the_launch_mob {
            wrong.push(format!(
                "meta/gating/pending {params}: {pending:?}, not {of_the_launch_mob:?}"
            ));
        }
        if audit != of_the_launch_mob {
            wrong.push(format!(
                "meta/gating/audit {params}: {audit:?}, not {of_the_launch_mob:?}"
            ));
        }
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a gating list without `mobs` (the second mob's entry is {of_second}) answers more \
         than the launch mob's members' entries:\n{}",
        wrong.join("\n")
    );
}

/// With every mob selected the host's gating lists answer the entries of
/// each mob the caller may address, and an entry names its member by its
/// mob and its name: the second mob's entry carries that mob's id beside
/// the name, the launch mob's carries the name alone. The selector never
/// stands beside a `mob_id` there either.
#[tokio::test]
async fn with_every_mob_selected_a_gating_list_answers_each_mob_its_caller_may_address() {
    let console = console_with_gating().await;
    let of_launch = pending_entry_of(&console, None).await;
    let of_second = pending_entry_of(&console, Some(&console.second_mob)).await;
    let mut of_both = vec![of_launch.clone(), of_second.clone()];
    of_both.sort();

    let mut wrong = Vec::new();
    let every_mob = json!({ "mobs": "*" });
    let pending = asked(&console, "meta/gating/pending", every_mob.clone()).await;
    let audit = asked(&console, "meta/gating/audit", every_mob).await;
    if pending.get("result").is_none() || pending_ids(&pending, "/result/pending") != of_both {
        wrong.push(format!("meta/gating/pending: not both entries: {pending}"));
    } else {
        let origin_of = |id: &str| {
            pending["result"]["pending"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|entry| entry["pending_id"].as_str() == Some(id))
                .map(|entry| entry["origin"].clone())
                .unwrap_or(Value::Null)
        };
        if origin_of(&of_second) != json!({ "mob_id": console.second_mob, "identity": "alice" }) {
            wrong.push(format!(
                "the second mob's entry does not name its member by its mob and its name: {}",
                origin_of(&of_second)
            ));
        }
        if origin_of(&of_launch) != json!({ "identity": "alice" }) {
            wrong.push(format!(
                "the launch mob's entry does not name its member by its name alone: {}",
                origin_of(&of_launch)
            ));
        }
    }
    if audit.get("result").is_none() || pending_ids(&audit, "/result/entries") != of_both {
        wrong.push(format!("meta/gating/audit: not both entries: {audit}"));
    }
    for method in ["meta/gating/pending", "meta/gating/audit"] {
        let beside_a_mob = asked(
            &console,
            method,
            json!({ "mobs": "*", "mob_id": console.second_mob }),
        )
        .await;
        if beside_a_mob["error"]["code"] != json!(-32602) {
            wrong.push(format!(
                "{method}: `mobs` beside a `mob_id` is not refused: {beside_a_mob}"
            ));
        }
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a gating list with every mob selected does not answer each addressable mob:\n{}",
        wrong.join("\n")
    );
}

/// The selector is a parameter of the host's lists that name members, and
/// of no method that names one entry by its id: a list request that
/// carries `mobs=*` is not refused for it, and a request that decides,
/// dismisses or reads one entry is.
#[tokio::test]
async fn the_selector_is_a_parameter_of_the_hosts_lists_and_of_no_request_about_one_entry() {
    let console = console_with_gating().await;
    let refused_for_the_selector = |answer: &Value| {
        answer["error"]["code"] == json!(-32602)
            && answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("mobs"))
    };
    let mut wrong = Vec::new();
    for method in [
        "meta/console/attention",
        "meta/console/audit",
        "meta/gating/audit",
        "meta/gating/pending",
    ] {
        let answer = asked(&console, method, json!({ "mobs": "*" })).await;
        if refused_for_the_selector(&answer) {
            wrong.push(format!("{method}: refuses the selector: {answer}"));
        }
        let other_value = asked(&console, method, json!({ "mobs": "second" })).await;
        if !refused_for_the_selector(&other_value) {
            wrong.push(format!(
                "{method}: takes a `mobs` that is not every mob: {other_value}"
            ));
        }
    }
    for (method, params) in [
        (
            "meta/console/attention/dismiss",
            json!({ "mobs": "*", "item_id": "no-such-item" }),
        ),
        (
            "meta/decisions/get",
            json!({ "mobs": "*", "decision_id": "no-such-decision" }),
        ),
        // A decision names no member and no mob: its list is no list of
        // the host that names members, and takes no selector.
        ("meta/decisions/list", json!({ "mobs": "*" })),
        (
            "meta/gating/decide",
            json!({ "mobs": "*", "pending_id": "no-such-entry", "decision": "approve" }),
        ),
    ] {
        let answer = asked(&console, method, params).await;
        if !refused_for_the_selector(&answer) {
            wrong.push(format!("{method}: takes the selector: {answer}"));
        }
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the selector is not a parameter of exactly the host's lists:\n{}",
        wrong.join("\n")
    );
}

/// One row of the console audit log for a change that targeted the member
/// `alice` of the mob `mob` (none: a change that named no mob). Its number.
async fn audit_row_of(console: &TwoMobs, mob: Option<&str>) -> u64 {
    let mut target = serde_json::Map::new();
    target.insert("member_id".to_string(), json!("alice"));
    if let Some(mob) = mob {
        target.insert(
            crate::console_audit::TARGET_MOB_FIELD.to_string(),
            json!(mob),
        );
    }
    console
        .audit
        .as_ref()
        .expect("the fixture keeps an audit log")
        .record(crate::console_audit::ConsoleAuditEntry {
            actor: crate::console_audit::ConsoleAuditActor {
                kind: "person",
                principal: Some(ADMIN.to_string()),
                issuer: None,
                subject: None,
                assurance: None,
                login_scope_digest: None,
            },
            method: "meta/retire_member".to_string(),
            target,
            outcome: crate::console_audit::ConsoleAuditOutcome::Committed,
            code: None,
            kind: None,
            receipt: serde_json::Map::new(),
            truncated: false,
        })
        .await
        .expect("the row is written")
        .seq
}

/// The numbers of the rows `meta/console/audit` with `params` answers an
/// administrator; the answer itself when it is no page.
async fn audit_rows(console: &TwoMobs, params: Value) -> Result<Vec<u64>, Value> {
    let answer = asked(console, "meta/console/audit", params).await;
    let Some(records) = answer["result"]["records"].as_array() else {
        return Err(answer);
    };
    Ok(records
        .iter()
        .filter_map(|row| row["seq"].as_u64())
        .collect())
}

/// The console audit read is under the rule of the host's lists. A request
/// that does not select every mob is answered the launch mob's rows and no
/// row of a change that named another mob, even to an administrator, and a
/// mob named beside the method selects nothing; a row that names the
/// launch mob by its id is the launch mob's. With every mob selected it
/// answers the rows of each mob the caller may address, and never beside a
/// `mob_id`.
#[tokio::test]
async fn the_console_audit_read_answers_the_launch_mobs_rows_unless_every_mob_is_selected() {
    let console = console_with_gating().await;
    let of_launch = audit_row_of(&console, None).await;
    let of_launch_by_id = audit_row_of(&console, Some(&console.launch_mob)).await;
    let of_second = audit_row_of(&console, Some(&console.second_mob)).await;

    let mut wrong = Vec::new();
    for params in [json!({}), json!({ "mob_id": console.second_mob })] {
        let rows = audit_rows(&console, params.clone()).await;
        if rows != Ok(vec![of_launch, of_launch_by_id]) {
            wrong.push(format!(
                "{params}: {rows:?}, not the launch mob's rows {of_launch} and {of_launch_by_id}"
            ));
        }
    }
    let every_mob = audit_rows(&console, json!({ "mobs": "*" })).await;
    if every_mob != Ok(vec![of_launch, of_launch_by_id, of_second]) {
        wrong.push(format!(
            "with every mob selected: {every_mob:?}, not the three rows"
        ));
    }
    let beside_a_mob = asked(
        &console,
        "meta/console/audit",
        json!({ "mobs": "*", "mob_id": console.second_mob }),
    )
    .await;
    if beside_a_mob["error"]["code"] != json!(-32602) {
        wrong.push(format!(
            "`mobs` beside a `mob_id` is not refused: {beside_a_mob}"
        ));
    }
    console.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the console audit read is not under the rule of the host's lists:\n{}",
        wrong.join("\n")
    );
}

/// What the gating stream sends within `wait`, as text: empty when it sends
/// nothing.
async fn stream_text_within(
    body: &mut (impl futures::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
    wait: std::time::Duration,
    until: &str,
) -> String {
    use futures::StreamExt;

    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + wait;
    while !text.contains(until) {
        match tokio::time::timeout_at(deadline, body.next()).await {
            Ok(Some(Ok(chunk))) => text.push_str(&String::from_utf8_lossy(&chunk)),
            _ => break,
        }
    }
    text
}

/// The gating stream takes no selector and is the launch mob's: of the
/// entries members made it signals a change only for an entry of the launch
/// mob's members. An entry of another mob's member sends nothing, to an
/// administrator too: a caller that did not ask for every mob learns
/// nothing of the entries of another mob's members, a change signal
/// included. An entry of the launch mob's member is signalled as before.
#[tokio::test]
async fn the_gating_stream_signals_nothing_for_another_mobs_entry() {
    use tower::ServiceExt;

    let console = console_with_gating().await;
    let mut request = axum::http::Request::builder()
        .method("GET")
        .uri("/console/gating/stream")
        .body(axum::body::Body::empty())
        .expect("GET request");
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", super::mobs_selector_tests::token(ADMIN))
            .parse()
            .expect("header value"),
    );
    let response = console
        .app
        .clone()
        .oneshot(request)
        .await
        .expect("the stream opens");
    assert_eq!(response.status(), StatusCode::OK, "the gating stream");
    let mut body = response.into_body().into_data_stream();
    let opened = stream_text_within(
        &mut body,
        std::time::Duration::from_secs(10),
        "gating.resync_required",
    )
    .await;
    assert!(
        opened.contains("gating.resync_required"),
        "the stream opens with its first frame: {opened}"
    );

    pending_entry_of(&console, Some(&console.second_mob)).await;
    let of_another_mob = stream_text_within(
        &mut body,
        std::time::Duration::from_secs(3),
        "gating.changed",
    )
    .await;
    pending_entry_of(&console, None).await;
    let of_the_launch_mob = stream_text_within(
        &mut body,
        std::time::Duration::from_secs(10),
        "gating.changed",
    )
    .await;
    drop(body);
    console.shutdown().await;
    assert!(
        !of_another_mob.contains("gating.changed"),
        "the stream signals a change of another mob's entry: {of_another_mob}"
    );
    assert!(
        of_the_launch_mob.contains("gating.changed"),
        "the stream does not signal a change of the launch mob's entry: {of_the_launch_mob}"
    );
}
