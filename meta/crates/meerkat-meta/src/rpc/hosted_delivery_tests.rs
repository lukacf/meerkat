//! Rows for the delivery methods on a host that serves more than one mob.
//!
//! A resolution is made for one mob (`meta/routing/resolve` resolves over
//! the routes of the mob a request names) and names that mob; a delivery is
//! sent for one mob (the mob its request names, the launch mob when it
//! names none). A send takes a resolution of its own mob only.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::UnifiedRuntime;

/// A runtime over one mob, the default mob, with no identity plane and no
/// module.
async fn runtime() -> UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"hosted-delivery-rows-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds")
}

/// One request on the stdio dispatch. `None` when it did not answer in time.
async fn stdio(runtime: &UnifiedRuntime, method: &str, params: Value) -> Option<Value> {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let answer = tokio::time::timeout(
        Duration::from_secs(10),
        crate::rpc::handle_unified_rpc_json(runtime, &request, Duration::from_secs(1), None, None),
    )
    .await
    .ok()?;
    serde_json::from_str(&answer).ok()
}

/// A resolution as `meta/routing/resolve` answers it for the mob `mob_id`
/// (none for the launch mob).
fn resolution_for(mob_id: Option<&str>) -> Value {
    let mut resolution = json!({
        "route_id": "route-000001",
        "recipient": "alice",
        "channel": "notification",
        "sink": "webhook",
        "target_module": "delivery",
        "retry_max": 1,
        "backoff_ms": 250,
        "rate_limit_per_minute": 2,
    });
    if let Some(mob_id) = mob_id {
        resolution["mob_id"] = json!(mob_id);
    }
    resolution
}

/// A resolution as `meta/routing/resolve` answers it for the mob `mob_id`.
fn resolution_of(mob_id: &str) -> Value {
    resolution_for(Some(mob_id))
}

/// A delivery as the host's delivery service keeps it for the mob `mob_id`
/// (none for the launch mob). Built from its wire form, in which a mob is
/// one member more.
fn delivery_record(
    delivery_id: &str,
    recipient: &str,
    mob_id: Option<&str>,
) -> crate::runtime::DeliveryRecord {
    let mut record = json!({
        "delivery_id": delivery_id,
        "route_id": "route-000001",
        "recipient": recipient,
        "sink": "webhook",
        "target_module": "delivery",
        "payload": {"message": "hello"},
        "status": "sent",
        "attempts": [{"attempt": 1, "status": "sent", "backoff_ms": 0}],
        "first_attempt_ms": 1,
        "final_attempt_ms": 1,
        "idempotency_key": null,
        "sink_adapter": null,
    });
    if let Some(mob_id) = mob_id {
        record["mob_id"] = json!(mob_id);
    }
    serde_json::from_value(record).expect("a delivery record")
}

/// The params of a request that names `mob_id`, when it names one.
fn naming(mut params: Value, mob_id: Option<&str>) -> Value {
    if let Some(mob_id) = mob_id {
        params["mob_id"] = json!(mob_id);
    }
    params
}

/// A send that names no mob is the launch mob's. Handed a resolution that
/// names another mob the host serves, it is refused with a refusal of its
/// own kind that names both mobs, before anything else is judged: this host
/// loads no delivery module, and the answer is not that module's absence.
#[tokio::test]
async fn a_send_that_names_no_mob_refuses_a_resolution_of_a_hosted_mob_typed() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let answer = stdio(
        &host,
        "meta/delivery/send",
        json!({
            "resolution": resolution_of(&second_mob),
            "payload": {"message": "for the second mob"},
        }),
    )
    .await;
    host.shutdown().await;
    second.shutdown().await;

    let answer = answer.expect("the send answers in time");
    assert_eq!(answer["error"]["code"], json!(-32602), "{answer}");
    assert_eq!(
        answer["error"]["data"],
        json!({
            "kind": "resolution_of_another_mob",
            "resolution_mob_id": second_mob,
            "request_mob_id": launch_mob,
        }),
        "the refusal names its kind and both mobs: {answer}"
    );
    let message = answer["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&second_mob) && message.contains(&launch_mob),
        "the words name both mobs: {message}"
    );
}

/// A send for a mob takes a resolution of that mob and reaches the host's
/// delivery service: for the launch mob and for a second mob the host
/// serves alike. This host loads no delivery module, so the service's one
/// answer for that is the answer of both, whole; neither is a refusal of
/// its mob.
#[tokio::test]
async fn a_delivery_send_for_a_hosted_mob_reaches_the_delivery_service() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let second_mob = second.mob_id();
    let the_services_answer = json!({
        "code": -32602,
        "message": "Invalid params: delivery module is not loaded",
    });
    let mut wrong = Vec::new();
    for (whose, mob) in [
        ("the launch mob", None),
        ("the second mob", Some(second_mob.as_str())),
    ] {
        let answer = stdio(
            &host,
            "meta/delivery/send",
            naming(
                json!({
                    "resolution": resolution_for(mob),
                    "payload": {"message": "hello"},
                }),
                mob,
            ),
        )
        .await;
        if answer
            .as_ref()
            .is_none_or(|answer| answer["error"] != the_services_answer)
        {
            wrong.push(format!(
                "{whose}'s send is not the delivery service's answer without a delivery \
                 module: {answer:?}"
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A send that names a mob refuses a resolution of another mob, in both
/// directions, with the refusal that names both mobs: the second mob's send
/// with the launch mob's resolution, and the launch mob's send, named by
/// its id, with the second mob's.
#[tokio::test]
async fn a_send_for_one_mob_refuses_the_resolution_of_the_other() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let mut wrong = Vec::new();
    for (request_mob, resolution_mob) in [
        (second_mob.as_str(), None),
        (launch_mob.as_str(), Some(second_mob.as_str())),
    ] {
        let answer = stdio(
            &host,
            "meta/delivery/send",
            naming(
                json!({
                    "resolution": resolution_for(resolution_mob),
                    "payload": {"message": "hello"},
                }),
                Some(request_mob),
            ),
        )
        .await;
        let expected = json!({
            "kind": "resolution_of_another_mob",
            "resolution_mob_id": resolution_mob.unwrap_or(launch_mob.as_str()),
            "request_mob_id": request_mob,
        });
        if answer.as_ref().is_none_or(|answer| {
            answer["error"]["code"] != json!(-32602) || answer["error"]["data"] != expected
        }) {
            wrong.push(format!(
                "a send for {request_mob} with a resolution of {}: {answer:?}",
                resolution_mob.unwrap_or("the launch mob")
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The refusal's words say what each side names: the mob the resolution
/// names, or that it names none, and the mob the send is for. A resolution
/// that names no mob is not said to be made for a mob by its id, and a
/// resolution that names the launch mob by its id, sent for the launch mob,
/// is told that a resolution of the default mob names none.
#[tokio::test]
async fn the_refusal_of_another_mobs_resolution_says_what_each_side_names() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    let rule = "a send takes a resolution made for its own mob, and a resolution of the default \
                mob names no mob";
    let mut wrong = Vec::new();
    for (request_mob, resolution_mob, words) in [
        (
            Some(second_mob.as_str()),
            None,
            format!(
                "Invalid params: the resolution names no mob, and this send is for mob \
                 '{second_mob}'; {rule}"
            ),
        ),
        (
            None,
            Some(launch_mob.as_str()),
            format!(
                "Invalid params: the resolution names mob '{launch_mob}', and this send is for \
                 the default mob '{launch_mob}'; {rule}"
            ),
        ),
        (
            None,
            Some(second_mob.as_str()),
            format!(
                "Invalid params: the resolution names mob '{second_mob}', and this send is for \
                 the default mob '{launch_mob}'; {rule}"
            ),
        ),
    ] {
        let answer = stdio(
            &host,
            "meta/delivery/send",
            naming(
                json!({
                    "resolution": resolution_for(resolution_mob),
                    "payload": {"message": "hello"},
                }),
                request_mob,
            ),
        )
        .await;
        let said = answer
            .as_ref()
            .and_then(|answer| answer["error"]["message"].as_str())
            .unwrap_or_default();
        if said != words {
            wrong.push(format!(
                "a send for {request_mob:?} with a resolution that names {resolution_mob:?}: \
                 said {said:?}, not {words:?}"
            ));
        }
    }
    host.shutdown().await;
    second.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The delivery history is read for one mob: a read that names no mob, or
/// the launch mob's id, lists the launch mob's deliveries alone, and a read
/// that names a second mob the host serves lists that mob's alone. A
/// delivery of the second mob names its mob; one of the launch mob names
/// none.
#[tokio::test]
async fn the_delivery_history_of_each_mob_lists_its_own_deliveries() {
    let host = runtime().await;
    let second = host.serve_a_second_mob_in_tests(runtime().await);
    let launch_mob = host.mob_id();
    let second_mob = second.mob_id();
    {
        let module_runtime = host.module_runtime_handle();
        let mut module_runtime = module_runtime.lock().await;
        module_runtime.record_delivery_for_test(delivery_record("delivery-000001", "alice", None));
        module_runtime.record_delivery_for_test(delivery_record(
            "delivery-000002",
            "alice",
            Some(&second_mob),
        ));
    }
    let mut listed = Vec::new();
    for mob in [None, Some(launch_mob.as_str()), Some(second_mob.as_str())] {
        let answer = stdio(
            &host,
            "meta/delivery/history",
            naming(json!({"limit": 50}), mob),
        )
        .await;
        listed.push(answer.as_ref().and_then(|answer| {
            answer["result"]["deliveries"].as_array().map(|deliveries| {
                deliveries
                    .iter()
                    .map(|delivery| (delivery["delivery_id"].clone(), delivery["mob_id"].clone()))
                    .collect::<Vec<_>>()
            })
        }));
    }
    host.shutdown().await;
    second.shutdown().await;

    let the_launch_mobs = Some(vec![(json!("delivery-000001"), Value::Null)]);
    assert_eq!(
        listed,
        [
            the_launch_mobs.clone(),
            the_launch_mobs,
            Some(vec![(json!("delivery-000002"), json!(second_mob))]),
        ],
        "a read that names no mob, the launch mob, the second mob"
    );
}
