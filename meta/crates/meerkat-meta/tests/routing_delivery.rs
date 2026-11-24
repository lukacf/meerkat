#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::TestClient;
use meerkat_meta::runtime::{
    DeliveryHistoryRequest, DeliverySendError, DeliverySendRequest, RoutingResolution,
    RoutingResolveRequest,
};
use meerkat_meta::{
    DiscoverySpec, MetaConfig, ModuleConfig, PreSpawnData, RestartPolicy, UnifiedEvent,
    UnifiedRuntime, handle_meta_rpc_json, start_meta_runtime,
};
use meerkat_mob::MobDefinition;
use serde_json::{Value, json};

const BOUNDARY_ENV_KEY: &str = "META_MODULE_BOUNDARY";
const BOUNDARY_ENV_VALUE_MCP: &str = "mcp";
const DELAY_ON_ENV: &str = "META_PHASE_C_DELAY_ON";
const DELAY_MS_ENV: &str = "META_PHASE_C_DELAY_MS";

fn fixture_binary_path() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_mcp_fixture") {
        return PathBuf::from(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let binary_path = workspace_root
        .join("target")
        .join("debug")
        .join("mcp_fixture");
    if binary_path.exists() {
        return binary_path;
    }

    let status = Command::new("cargo")
        .args(["build", "-p", "meerkat-meta", "--bin", "mcp_fixture"])
        .current_dir(workspace_root)
        .status()
        .expect("build mcp_fixture");
    assert!(status.success(), "building mcp_fixture must succeed");
    binary_path
}

fn fixture_module(id: &str, fixture_binary: &std::path::Path) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: fixture_binary.display().to_string(),
        args: vec!["--module".to_string(), id.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn mcp_env(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env = vec![(
        BOUNDARY_ENV_KEY.to_string(),
        BOUNDARY_ENV_VALUE_MCP.to_string(),
    )];
    env.extend(
        extra
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
    );
    env
}

fn routing_delivery_config(
    router_extra_env: &[(&str, &str)],
    delivery_extra_env: &[(&str, &str)],
) -> MetaConfig {
    let fixture_binary = fixture_binary_path();
    MetaConfig {
        modules: vec![
            fixture_module("router", &fixture_binary),
            fixture_module("delivery", &fixture_binary),
        ],
        discovery: DiscoverySpec {
            namespace: "phase10".to_string(),
            modules: vec!["router".to_string(), "delivery".to_string()],
        },
        pre_spawn: vec![
            PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env(router_extra_env),
            },
            PreSpawnData {
                module_id: "delivery".to_string(),
                env: mcp_env(delivery_extra_env),
            },
        ],
    }
}

fn routing_delivery_runtime_with_env(
    router_extra_env: &[(&str, &str)],
    delivery_extra_env: &[(&str, &str)],
) -> meerkat_meta::MetaRuntimeHandle {
    let config = routing_delivery_config(router_extra_env, delivery_extra_env);

    start_meta_runtime(config, vec![], Duration::from_secs(2)).expect("runtime starts")
}

fn routing_delivery_runtime() -> meerkat_meta::MetaRuntimeHandle {
    routing_delivery_runtime_with_env(&[], &[])
}

fn parse_response(raw: &str) -> Value {
    serde_json::from_str(raw).expect("valid rpc response")
}

#[test]
fn phase0_contract_009_runtime_routes_and_delivery_history_project_without_host_invention() {
    let mut runtime = routing_delivery_runtime();

    let add_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-route-add","method":"meta/routing/routes/add","params":{"route":{"route_key":"vip-route","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery","retry_max":0,"backoff_ms":5,"rate_limit_per_minute":9}}}"#,
        Duration::from_secs(1),
    ));
    let listed_routes = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-route-list","method":"meta/routing/routes/list","params":{}}"#,
        Duration::from_secs(1),
    ));
    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-route-resolve","method":"meta/routing/resolve","params":{"recipient":"vip@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase0-delivery-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase0-contract-009",
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-delivery-history","method":"meta/delivery/history","params":{"recipient":"vip@example.com","limit":10}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        add_route["result"]["route"]["route_key"],
        json!("vip-route")
    );
    assert_eq!(
        listed_routes["result"]["routes"],
        json!([
            {
                "route_key":"vip-route",
                "recipient":"vip@example.com",
                "channel":"notification",
                "sink":"sms",
                "target_module":"delivery",
                "retry_max":0,
                "backoff_ms":5,
                "rate_limit_per_minute":9,
            }
        ])
    );
    assert_eq!(sent["result"]["status"], json!("sent"));
    assert_eq!(
        history["result"]["deliveries"]
            .as_array()
            .map_or(0, Vec::len),
        1
    );
    assert_eq!(
        history["result"]["deliveries"][0]["recipient"],
        json!("vip@example.com")
    );
    assert_eq!(history["result"]["deliveries"][0]["sink"], json!("sms"));
    assert_eq!(
        history["result"]["deliveries"][0]["target_module"],
        json!("delivery")
    );
    assert_eq!(
        history["result"]["deliveries"][0]["attempts"],
        json!([
            {
                "attempt": 1,
                "status": "sent",
                "backoff_ms": 0,
            }
        ])
    );
}

#[test]
fn phase0_contract_009_delivery_send_survives_slow_mcp_subprocess() {
    let mut runtime = routing_delivery_runtime_with_env(
        &[],
        &[
            (DELAY_ON_ENV, "call_tool:delivery.send"),
            (DELAY_MS_ENV, "1500"),
        ],
    );

    let add_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-slow-route-add","method":"meta/routing/routes/add","params":{"route":{"route_key":"slow-route","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery","retry_max":0,"backoff_ms":5,"rate_limit_per_minute":9}}}"#,
        Duration::from_secs(1),
    ));
    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-slow-route-resolve","method":"meta/routing/resolve","params":{"recipient":"vip@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase0-slow-delivery-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase0-contract-009-slow",
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-slow-delivery-history","method":"meta/delivery/history","params":{"recipient":"vip@example.com","limit":10}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        add_route["result"]["route"]["route_key"],
        json!("slow-route")
    );
    assert!(
        sent["error"].is_null(),
        "slow delivery should not time out under the core MCP budget: {sent}"
    );
    assert_eq!(sent["result"]["status"], json!("sent"));
    assert_eq!(
        history["result"]["deliveries"]
            .as_array()
            .map_or(0, Vec::len),
        1
    );
}

/// A resolution for `recipient` over the routes of the mob `mob_id` names:
/// the module runtime is the host's, for all its mobs, and a resolve names
/// the mob it is for (none is the host's default mob).
fn resolve_for_the_mob(
    runtime: &mut meerkat_meta::MetaRuntimeHandle,
    mob_id: &str,
) -> RoutingResolution {
    runtime
        .resolve_routing(RoutingResolveRequest {
            mob_id: Some(meerkat_mob::MobId::from(mob_id)),
            recipient: "approvals@example.com".to_string(),
            channel: Some("notification".to_string()),
            retry_max: None,
            backoff_ms: None,
            rate_limit_per_minute: None,
        })
        .expect("a resolve for another mob of the host is served")
}

/// The deliveries the runtime keeps for the default mob and for the mob
/// `second`, each mob's newest last. The requests are built from their wire
/// form, in which a mob is one member more.
fn recorded_deliveries(runtime: &meerkat_meta::MetaRuntimeHandle) -> Vec<Value> {
    [None, Some("second")]
        .into_iter()
        .flat_map(|mob_id| deliveries_of(runtime, mob_id))
        .collect()
}

/// The deliveries the runtime keeps for the mob `mob_id` (none for the
/// default mob), newest last.
fn deliveries_of(runtime: &meerkat_meta::MetaRuntimeHandle, mob_id: Option<&str>) -> Vec<Value> {
    let mut request = json!({"limit": 200});
    if let Some(mob_id) = mob_id {
        request["mob_id"] = json!(mob_id);
    }
    let request: DeliveryHistoryRequest =
        serde_json::from_value(request).expect("a history request");
    runtime
        .delivery_history(request)
        .deliveries
        .iter()
        .map(|record| serde_json::to_value(record).expect("a delivery record as JSON"))
        .collect()
}

/// A send for the mob `mob_id` (none for the default mob), built from its
/// wire form.
fn send_for(
    runtime: &mut meerkat_meta::MetaRuntimeHandle,
    mob_id: Option<&str>,
    resolution: &RoutingResolution,
    idempotency_key: Option<&str>,
) -> Result<Value, DeliverySendError> {
    let mut request = json!({
        "resolution": resolution,
        "payload": {"message": "hello"},
    });
    if let Some(mob_id) = mob_id {
        request["mob_id"] = json!(mob_id);
    }
    if let Some(idempotency_key) = idempotency_key {
        request["idempotency_key"] = json!(idempotency_key);
    }
    let request: DeliverySendRequest = serde_json::from_value(request).expect("a send request");
    runtime
        .send_delivery(request)
        .map(|record| serde_json::to_value(record).expect("a delivery record as JSON"))
}

/// A resolution over the routes of the default mob.
fn resolve_for_the_default_mob(
    runtime: &mut meerkat_meta::MetaRuntimeHandle,
    rate_limit_per_minute: Option<u32>,
) -> RoutingResolution {
    runtime
        .resolve_routing(RoutingResolveRequest {
            mob_id: None,
            recipient: "approvals@example.com".to_string(),
            channel: Some("notification".to_string()),
            retry_max: None,
            backoff_ms: None,
            rate_limit_per_minute,
        })
        .expect("a resolve for the default mob is served")
}

/// How many deliveries the runtime's event stream reports.
fn delivery_send_events(runtime: &meerkat_meta::MetaRuntimeHandle) -> usize {
    runtime
        .merged_events()
        .iter()
        .filter(|envelope| {
            matches!(
                &envelope.event,
                UnifiedEvent::Module(event)
                    if event.module == "delivery" && event.event_type == "send"
            )
        })
        .count()
}

/// A resolution is made for one mob of the host, and a send that names no
/// mob is the default mob's. Such a send refuses a resolution that was made
/// for another mob: the delivery module is not called, nothing is recorded
/// and no delivery is reported. The resolution itself names its mob.
#[test]
fn a_resolution_made_for_another_mob_is_refused_by_a_send_for_the_default_mob() {
    let mut runtime = routing_delivery_runtime();
    let resolution = resolve_for_the_mob(&mut runtime, "second");
    let sent = send_for(&mut runtime, None, &resolution, Some("one-key"));
    let recorded = recorded_deliveries(&runtime);
    let send_events = delivery_send_events(&runtime);
    runtime.shutdown();

    let answered = match &sent {
        Ok(record) => format!("sent as {}", record["delivery_id"]),
        Err(error) => format!("{error:?}"),
    };
    assert!(
        sent.is_err() && answered.contains("ResolutionOfAnotherMob"),
        "a send for the default mob took a resolution made for the mob `second`: {answered}"
    );
    assert!(
        recorded.is_empty(),
        "the refused send is in the delivery history: {recorded:?}"
    );
    assert_eq!(send_events, 0, "the refused send is reported as a delivery");
    assert_eq!(
        serde_json::to_value(&resolution).expect("a resolution as JSON")["mob_id"],
        json!("second"),
        "the resolution names the mob it was made for"
    );
}

/// A caller cannot turn another mob's resolution into the default mob's by
/// taking the mob out of it: the runtime's own copy still names the mob, so
/// the edited resolution is refused as a forged one, like any resolution
/// that differs from the one the runtime made.
#[test]
fn a_resolution_with_its_mob_removed_is_refused_as_forged() {
    let mut runtime = routing_delivery_runtime();
    let resolution = resolve_for_the_mob(&mut runtime, "second");
    let mut edited = serde_json::to_value(&resolution).expect("a resolution as JSON");
    edited
        .as_object_mut()
        .expect("a resolution is an object")
        .remove("mob_id");
    let edited: RoutingResolution =
        serde_json::from_value(edited).expect("a resolution without its mob still parses");
    let sent = send_for(&mut runtime, None, &edited, None);
    let recorded = recorded_deliveries(&runtime);
    runtime.shutdown();

    assert!(
        matches!(sent, Err(DeliverySendError::ForgedResolution)),
        "a resolution of the mob `second` with its mob removed: {sent:?}"
    );
    assert!(
        recorded.is_empty(),
        "the refused send is in the delivery history: {recorded:?}"
    );
}

/// Two mobs of one host each send under the same idempotency key, each with
/// a resolution of its own: two deliveries, each recorded for its mob and
/// read back in that mob's history alone. A delivery of the default mob
/// names no mob; one of another mob names it. The same send again is the
/// delivery already made. A send for one mob with the other's resolution is
/// refused.
#[test]
fn two_mobs_send_under_one_idempotency_key_and_keep_their_own_deliveries_and_histories() {
    let mut runtime = routing_delivery_runtime();
    let of_the_default = resolve_for_the_default_mob(&mut runtime, None);
    let of_the_second = resolve_for_the_mob(&mut runtime, "second");
    let first = send_for(&mut runtime, None, &of_the_default, Some("one-key"));
    let second = send_for(
        &mut runtime,
        Some("second"),
        &of_the_second,
        Some("one-key"),
    );
    let again = send_for(
        &mut runtime,
        Some("second"),
        &of_the_second,
        Some("one-key"),
    );
    let crossed = send_for(&mut runtime, Some("second"), &of_the_default, None);
    let default_history = deliveries_of(&runtime, None);
    let second_history = deliveries_of(&runtime, Some("second"));
    let send_events = delivery_send_events(&runtime);
    runtime.shutdown();

    let first = first.expect("the default mob's send");
    let second = second.expect("the second mob's send with a resolution of its own");
    let again = again.expect("the second mob's send again under its key");
    assert_ne!(
        first["delivery_id"], second["delivery_id"],
        "one key under two mobs is two deliveries"
    );
    assert_eq!(again, second, "the same send again is the same delivery");
    assert!(
        first.get("mob_id").is_none(),
        "a delivery of the default mob names no mob: {first}"
    );
    assert_eq!(second["mob_id"], json!("second"), "{second}");
    assert_eq!(send_events, 2, "two deliveries were made");
    let crossed = crossed.map_or_else(
        |error| format!("{error:?}"),
        |record| format!("sent: {record}"),
    );
    assert!(
        crossed.contains("ResolutionOfAnotherMob"),
        "the second mob's send with the default mob's resolution: {crossed}"
    );
    assert_eq!(default_history, [first], "the default mob's history");
    assert_eq!(second_history, [second], "the second mob's history");
}

/// The delivery module is told the mob of a delivery: the call it is made
/// with names the mob, at the top and in the resolution, for a mob other
/// than the default mob, and names none for the default mob.
#[test]
fn the_delivery_module_is_told_the_mob_of_a_hosted_mobs_send() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let log_path = scratch.path().join("delivery-calls.log");
    let log = log_path.display().to_string();
    let mut runtime =
        routing_delivery_runtime_with_env(&[], &[("META_PHASE_C_LOG_PATH", log.as_str())]);
    let of_the_default = resolve_for_the_default_mob(&mut runtime, None);
    let of_the_second = resolve_for_the_mob(&mut runtime, "second");
    let first = send_for(&mut runtime, None, &of_the_default, None);
    let second = send_for(&mut runtime, Some("second"), &of_the_second, None);
    runtime.shutdown();

    first.expect("the default mob's send");
    second.expect("the second mob's send with a resolution of its own");
    let calls: Vec<Value> = std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.strip_prefix("delivery:call:delivery.send:"))
        .map(|arguments| serde_json::from_str(arguments).expect("a call's arguments as JSON"))
        .collect();
    assert_eq!(
        calls.len(),
        2,
        "the delivery module was called twice: {calls:?}"
    );
    assert!(
        calls[0].get("mob_id").is_none() && calls[0]["resolution"].get("mob_id").is_none(),
        "the default mob's call names no mob: {}",
        calls[0]
    );
    assert_eq!(calls[1]["mob_id"], json!("second"), "{}", calls[1]);
    assert_eq!(
        calls[1]["resolution"]["mob_id"],
        json!("second"),
        "{}",
        calls[1]
    );
}

/// What one mob sends never drops another mob's deliveries, so a keyed
/// retry of one mob is never sent again because another mob was busy. The
/// second mob sends once under a key; the default mob then makes more
/// deliveries than a mob's history keeps; the second mob's send again under
/// its key is the delivery already made, and its history still holds it.
#[test]
fn a_keyed_retry_of_one_mob_is_not_sent_again_because_another_mob_was_busy() {
    const A_MOBS_HISTORY: usize = 200;
    const BUSY: usize = A_MOBS_HISTORY + 5;
    let mut runtime = routing_delivery_runtime();
    let of_the_second = resolve_for_the_mob(&mut runtime, "second");
    let kept = send_for(
        &mut runtime,
        Some("second"),
        &of_the_second,
        Some("kept-key"),
    );
    let of_the_default = resolve_for_the_default_mob(&mut runtime, Some(10_000));
    let busy = (0..BUSY)
        .filter(|_| send_for(&mut runtime, None, &of_the_default, None).is_ok())
        .count();
    let retried = send_for(
        &mut runtime,
        Some("second"),
        &of_the_second,
        Some("kept-key"),
    );
    let second_history = deliveries_of(&runtime, Some("second"));
    let default_history = deliveries_of(&runtime, None);
    let send_events = delivery_send_events(&runtime);
    runtime.shutdown();

    let kept = kept.expect("the second mob's send under its key");
    let retried = retried.expect("the second mob's send again under its key");
    assert_eq!(busy, BUSY, "the default mob's deliveries were made");
    assert_eq!(
        retried, kept,
        "the retry is the delivery already made, not a new one"
    );
    assert_eq!(
        send_events,
        BUSY + 1,
        "the retry made no delivery of its own"
    );
    assert_eq!(
        default_history.len(),
        A_MOBS_HISTORY,
        "the default mob keeps its newest deliveries"
    );
    assert_eq!(second_history, [kept], "the second mob's history");
}

/// The route `vip-route` of the mob `mob_id` names (none for the default
/// mob), built from its wire form. Its recipient is not the one the rows
/// resolve for, so no resolve of a row takes it.
fn vip_route_of(mob_id: Option<&str>) -> meerkat_meta::RuntimeRoute {
    let mut route = json!({
        "route_key": "vip-route",
        "recipient": "vip@example.com",
        "channel": "notification",
        "sink": "sms",
        "target_module": "delivery",
    });
    if let Some(mob_id) = mob_id {
        route["mob_id"] = json!(mob_id);
    }
    serde_json::from_value(route).expect("a route")
}

/// The mobs whose route `vip-route` the runtime keeps (none is the default
/// mob), sorted.
fn mobs_with_the_vip_route(runtime: &meerkat_meta::MetaRuntimeHandle) -> Vec<Option<String>> {
    let mut mobs: Vec<Option<String>> = runtime
        .list_runtime_routes()
        .iter()
        .filter(|route| route.route_key == "vip-route")
        .map(|route| {
            serde_json::to_value(route).expect("a route as JSON")["mob_id"]
                .as_str()
                .map(str::to_string)
        })
        .collect();
    mobs.sort();
    mobs
}

/// One call forgets what the host keeps of one mob, for a mob that is
/// destroyed: its routes, the resolutions made for it, its deliveries and
/// the keys they were sent under. A mob created later under the same id
/// starts with none of it: no route of the old mob is listed, the old
/// mob's resolution is refused as one the host does not know, the history
/// is empty, and a send under the old mob's key is a new delivery. The
/// default mob and a third mob keep what is theirs.
#[test]
fn a_forgotten_mob_leaves_no_route_no_resolution_and_no_delivery_behind() {
    let mut runtime = routing_delivery_runtime();
    for mob_id in [None, Some("second"), Some("third")] {
        runtime
            .add_runtime_route(vip_route_of(mob_id))
            .expect("the route is added");
    }
    let of_the_default = resolve_for_the_default_mob(&mut runtime, None);
    let of_the_second = resolve_for_the_mob(&mut runtime, "second");
    let of_the_third = resolve_for_the_mob(&mut runtime, "third");
    let default_before = send_for(&mut runtime, None, &of_the_default, Some("one-key"));
    let second_before = send_for(
        &mut runtime,
        Some("second"),
        &of_the_second,
        Some("one-key"),
    );
    let third_before = send_for(&mut runtime, Some("third"), &of_the_third, Some("one-key"));

    runtime.forget_mob(&meerkat_mob::MobId::from("second"));

    let routes = mobs_with_the_vip_route(&runtime);
    let history_of_the_second = deliveries_of(&runtime, Some("second"));
    let with_the_old_resolution = send_for(
        &mut runtime,
        Some("second"),
        &of_the_second,
        Some("one-key"),
    );
    let a_new_resolution = resolve_for_the_mob(&mut runtime, "second");
    let second_after = send_for(
        &mut runtime,
        Some("second"),
        &a_new_resolution,
        Some("one-key"),
    );
    let default_again = send_for(&mut runtime, None, &of_the_default, Some("one-key"));
    let third_again = send_for(&mut runtime, Some("third"), &of_the_third, Some("one-key"));
    let default_history = deliveries_of(&runtime, None);
    let third_history = deliveries_of(&runtime, Some("third"));
    runtime.shutdown();

    let default_before = default_before.expect("the default mob's send");
    let second_before = second_before.expect("the second mob's send");
    let third_before = third_before.expect("the third mob's send");
    assert_eq!(
        routes,
        [None, Some("third".to_string())],
        "the forgotten mob's route is gone, and the default mob's and the third mob's stay"
    );
    assert!(
        history_of_the_second.is_empty(),
        "the forgotten mob's deliveries are gone: {history_of_the_second:?}"
    );
    assert!(
        matches!(
            with_the_old_resolution,
            Err(DeliverySendError::UnknownRouteId(_))
        ),
        "a resolution made for the forgotten mob: {with_the_old_resolution:?}"
    );
    let second_after = second_after.expect("a send of the mob under its id again");
    assert_ne!(
        second_after["delivery_id"], second_before["delivery_id"],
        "a send under the forgotten mob's key is a new delivery"
    );
    assert_eq!(
        default_again.expect("the default mob's send again"),
        default_before,
        "the default mob's keyed send is the delivery already made"
    );
    assert_eq!(
        third_again.expect("the third mob's send again"),
        third_before,
        "the third mob's keyed send is the delivery already made"
    );
    assert_eq!(
        default_history,
        [default_before],
        "the default mob's history"
    );
    assert_eq!(third_history, [third_before], "the third mob's history");
}

/// The resolution `resolution` with its mob edited: named `mob`, or taken
/// out.
fn with_its_mob_edited(resolution: &RoutingResolution, mob: Option<&str>) -> RoutingResolution {
    let mut edited = serde_json::to_value(resolution).expect("a resolution as JSON");
    let members = edited.as_object_mut().expect("a resolution is an object");
    match mob {
        Some(mob) => {
            members.insert("mob_id".to_string(), json!(mob));
        }
        None => {
            members.remove("mob_id");
        }
    }
    serde_json::from_value(edited).expect("an edited resolution still parses")
}

/// A resolution whose mob a caller edited is refused whatever mob it is
/// sent for, with a key or without: as a resolution of another mob where
/// the edited mob is not the send's, and as one that does not match the
/// host's own where it is. A key under which a delivery was made changes
/// neither answer while the payload is that delivery's; with another
/// payload the key's own refusal comes first. Nothing is sent or recorded.
#[test]
fn a_resolution_whose_mob_was_edited_is_refused_whatever_mob_it_is_sent_for() {
    let mut runtime = routing_delivery_runtime();
    let of_the_default = resolve_for_the_default_mob(&mut runtime, None);
    let of_the_second = resolve_for_the_mob(&mut runtime, "second");
    let kept_default = send_for(&mut runtime, None, &of_the_default, Some("one-key"));
    let kept_second = send_for(
        &mut runtime,
        Some("second"),
        &of_the_second,
        Some("one-key"),
    );
    let taken_out = with_its_mob_edited(&of_the_second, None);
    let put_in = with_its_mob_edited(&of_the_default, Some("second"));
    let changed = with_its_mob_edited(&of_the_second, Some("third"));

    let cases = [
        (
            "the second mob's with its mob taken out, sent for the second mob",
            &taken_out,
            Some("second"),
            "ResolutionOfAnotherMob",
        ),
        (
            "the second mob's with its mob taken out, sent for no mob",
            &taken_out,
            None,
            "ForgedResolution",
        ),
        (
            "the default mob's with a mob put in, sent for that mob",
            &put_in,
            Some("second"),
            "ForgedResolution",
        ),
        (
            "the default mob's with a mob put in, sent for no mob",
            &put_in,
            None,
            "ResolutionOfAnotherMob",
        ),
        (
            "the second mob's with its mob changed, sent for the new mob",
            &changed,
            Some("third"),
            "ForgedResolution",
        ),
        (
            "the second mob's with its mob changed, sent for its own mob",
            &changed,
            Some("second"),
            "ResolutionOfAnotherMob",
        ),
    ];
    let mut wrong = Vec::new();
    for (what, resolution, mob, refusal) in cases {
        for key in [None, Some("one-key")] {
            let answered = match send_for(&mut runtime, mob, resolution, key) {
                Ok(record) => format!("sent as {}", record["delivery_id"]),
                Err(error) => format!("{error:?}"),
            };
            if !answered.starts_with(refusal) {
                wrong.push(format!("{what}, key {key:?}: {answered}, not {refusal}"));
            }
        }
    }
    let with_another_payload: DeliverySendRequest = serde_json::from_value(json!({
        "mob_id": "second",
        "resolution": put_in,
        "payload": {"message": "another"},
        "idempotency_key": "one-key",
    }))
    .expect("a send request");
    let with_another_payload = runtime.send_delivery(with_another_payload);
    let default_history = deliveries_of(&runtime, None);
    let second_history = deliveries_of(&runtime, Some("second"));
    let third_history = deliveries_of(&runtime, Some("third"));
    let send_events = delivery_send_events(&runtime);
    runtime.shutdown();

    assert!(
        wrong.is_empty(),
        "an edited resolution was not refused as expected:\n{}",
        wrong.join("\n")
    );
    assert!(
        matches!(
            with_another_payload,
            Err(DeliverySendError::IdempotencyPayloadMismatch)
        ),
        "under a used key with another payload: {with_another_payload:?}"
    );
    assert_eq!(
        default_history,
        [kept_default.expect("the default mob's send")],
        "the default mob's history"
    );
    assert_eq!(
        second_history,
        [kept_second.expect("the second mob's send")],
        "the second mob's history"
    );
    assert!(
        third_history.is_empty(),
        "a third mob has no delivery: {third_history:?}"
    );
    assert_eq!(send_events, 2, "only the two real sends are reported");
}

/// An approval for the action `action`, asked with a notice to the approvals
/// address, at a host's action boundary: for a member the origin `origin`
/// names (its wire form), or for no member. The pending id of the request.
fn approval_asked(
    runtime: &mut meerkat_meta::MetaRuntimeHandle,
    action: &str,
    origin: Option<Value>,
) -> String {
    let request = serde_json::from_value(json!({
        "action": action,
        "actor_id": "alice",
        "risk_tier": "r3",
        "requested_approver": "bob",
        "approval_recipient": "approvals@example.com",
        "approval_channel": "notification",
    }))
    .expect("an approval request");
    let origin = origin.map(|origin| serde_json::from_value(origin).expect("an origin"));
    runtime
        .evaluate_gating_action_with_origin(request, origin)
        .pending_id
        .expect("the action waits for an approval")
}

/// The pending ids the approval notices among `deliveries` name, in order.
fn approvals_notified(deliveries: &[Value]) -> Vec<String> {
    deliveries
        .iter()
        .filter(|record| record["payload"]["kind"] == json!("gating_approval_request"))
        .filter_map(|record| record["payload"]["pending_id"].as_str().map(str::to_string))
        .collect()
}

/// The notice of an approval is a delivery of the mob its action came from.
/// Three approvals are asked on one host: one for no member, one for a
/// member of the default mob, one for a member of the mob `second`. The
/// first two notices are the default mob's deliveries, with the members a
/// notice always had and no mob; the third is the mob `second`'s delivery,
/// names that mob, and is in no other mob's history.
#[test]
fn an_approval_notice_is_a_delivery_of_the_mob_its_action_came_from() {
    let mut runtime = routing_delivery_runtime();
    let for_no_member = approval_asked(&mut runtime, "publish_release", None);
    let for_a_default_member = approval_asked(
        &mut runtime,
        "publish_notes",
        Some(json!({"identity": "alice"})),
    );
    let for_a_second_member = approval_asked(
        &mut runtime,
        "publish_report",
        Some(json!({"identity": "alice", "mob_id": "second"})),
    );
    let of_the_default = deliveries_of(&runtime, None);
    let of_the_second = deliveries_of(&runtime, Some("second"));
    runtime.shutdown();

    assert_eq!(
        approvals_notified(&of_the_default),
        [for_no_member, for_a_default_member],
        "the default mob's deliveries hold the notices of its own approvals: {of_the_default:?}"
    );
    assert_eq!(
        approvals_notified(&of_the_second),
        [for_a_second_member],
        "the second mob's deliveries hold the notice of its member's approval: {of_the_second:?}"
    );
    for record in &of_the_default {
        let members = |value: &Value| -> Vec<String> {
            value
                .as_object()
                .map(|object| object.keys().cloned().collect())
                .unwrap_or_default()
        };
        assert_eq!(
            members(record),
            [
                "attempts",
                "delivery_id",
                "final_attempt_ms",
                "first_attempt_ms",
                "idempotency_key",
                "payload",
                "recipient",
                "route_id",
                "sink",
                "sink_adapter",
                "status",
                "target_module"
            ],
            "a notice of the default mob has the members it always had: {record}"
        );
        assert_eq!(
            members(&record["payload"]),
            [
                "action",
                "action_id",
                "actor_id",
                "deadline_at_ms",
                "kind",
                "pending_id",
                "requested_approver",
                "risk_tier"
            ],
            "its payload has the members it always had: {record}"
        );
    }
    assert_eq!(
        of_the_second
            .iter()
            .map(|record| record["mob_id"].clone())
            .collect::<Vec<_>>(),
        [json!("second")],
        "the second mob's notice names its mob"
    );
}

/// The approvals the runtime holds, in the order they were asked: the
/// pending id of each and the mob its origin names (none for the default
/// mob).
fn approvals_waiting(
    runtime: &mut meerkat_meta::MetaRuntimeHandle,
) -> Vec<(String, Option<String>)> {
    runtime
        .list_gating_pending()
        .into_iter()
        .map(|entry| {
            let mob = entry
                .origin
                .and_then(|origin| origin.mob_id)
                .map(|mob_id| mob_id.as_str().to_string());
            (entry.pending_id, mob)
        })
        .collect()
}

/// A forgotten mob leaves no approval of its members waiting. One call
/// forgets what the host keeps of one mob, for a mob that is destroyed, and
/// the host withdraws the approvals it still holds for that mob's members:
/// a mob created later under the same id is asked for none of them. Three
/// approvals wait on one host, for a member `alice` of the default mob, of
/// the mob `second` and of the mob `third`. After the mob `second` is
/// forgotten its approval no longer waits: a decision of it is refused as
/// one of a request that is not pending, and the audit says the host
/// withdrew it and still names its mob. The other two still wait.
#[test]
fn a_forgotten_mob_leaves_no_approval_of_its_members_waiting() {
    let mut runtime = routing_delivery_runtime();
    let of_the_default = approval_asked(
        &mut runtime,
        "publish_notes",
        Some(json!({"identity": "alice"})),
    );
    let of_the_second = approval_asked(
        &mut runtime,
        "publish_report",
        Some(json!({"identity": "alice", "mob_id": "second"})),
    );
    let of_the_third = approval_asked(
        &mut runtime,
        "publish_plan",
        Some(json!({"identity": "alice", "mob_id": "third"})),
    );
    let waiting_before = approvals_waiting(&mut runtime);

    runtime.forget_mob(&meerkat_mob::MobId::from("second"));

    let waiting_after = approvals_waiting(&mut runtime);
    let decided = runtime.decide_gating_action(
        serde_json::from_value(json!({
            "pending_id": of_the_second,
            "approver_id": "bob",
            "decision": "approve",
        }))
        .expect("a decision"),
    );
    let withdrawals: Vec<Value> = runtime
        .gating_audit_entries(usize::MAX)
        .into_iter()
        .filter(|entry| entry.event_type == "request_withdrawn")
        .map(|entry| {
            json!({
                "pending_id": entry.pending_id,
                "withdrawn_by": entry.detail["withdrawn_by"],
                "mob_id": entry.detail["origin"]["mob_id"],
            })
        })
        .collect();
    runtime.shutdown();

    assert_eq!(
        waiting_before,
        [
            (of_the_default.clone(), None),
            (of_the_second.clone(), Some("second".to_string())),
            (of_the_third.clone(), Some("third".to_string())),
        ],
        "the three approvals waited before the mob was forgotten"
    );
    assert_eq!(
        waiting_after,
        [
            (of_the_default, None),
            (of_the_third, Some("third".to_string())),
        ],
        "the forgotten mob's approval no longer waits, and the other two do"
    );
    assert_eq!(
        decided.as_ref().err().map(ToString::to_string),
        Some(format!("unknown pending id: {of_the_second}")),
        "a decision of it is refused as one of a request that is not pending: {decided:?}"
    );
    assert_eq!(
        withdrawals,
        [json!({
            "pending_id": of_the_second,
            "withdrawn_by": "host",
            "mob_id": "second",
        })],
        "the audit says the host withdrew it and names its mob"
    );
}

/// The deliveries the host `runtime` keeps for the mob `mob_id` (none for
/// its default mob), as JSON.
async fn host_deliveries_of(runtime: &UnifiedRuntime, mob_id: Option<&str>) -> Vec<Value> {
    let mut request = json!({"limit": 200});
    if let Some(mob_id) = mob_id {
        request["mob_id"] = json!(mob_id);
    }
    let request: DeliveryHistoryRequest =
        serde_json::from_value(request).expect("a history request");
    runtime
        .delivery_history(request)
        .await
        .deliveries
        .iter()
        .map(|record| serde_json::to_value(record).expect("a delivery record as JSON"))
        .collect()
}

/// An origin may name the host's default mob by its id: it is the origin
/// that names no mob. A host of one mob has a route for the approvals
/// address, and an approval is asked at its action boundary for a member
/// whose origin names that mob by its id. The notice is among the default
/// mob's deliveries, sent over that route and naming no mob; no delivery is
/// kept under the mob's id; and the request the host keeps names no mob.
#[tokio::test(flavor = "multi_thread")]
async fn an_approval_for_an_origin_that_names_the_default_mob_by_its_id_is_the_default_mobs() {
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"delivery-default-by-id-{}\"\n\n[profiles.lead]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\n\n[profiles.lead.tools]\ncomms = true\n",
        std::process::id()
    ))
    .expect("a mob definition");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition)
            .module_config(routing_delivery_config(&[], &[]))
            .default_llm_client(Arc::new(TestClient::default()))
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("a host with the router and the delivery modules");
    let default_mob = runtime.mob_id();
    runtime
        .add_runtime_route(
            serde_json::from_value(json!({
                "route_key": "approvals-route",
                "recipient": "approvals@example.com",
                "channel": "notification",
                "sink": "sms",
                "target_module": "delivery",
                "retry_max": 0,
                "backoff_ms": 5,
                "rate_limit_per_minute": 9,
            }))
            .expect("a route"),
        )
        .await
        .expect("the default mob's route is added");
    let pending_id = runtime
        .evaluate_gating_action_with_origin(
            serde_json::from_value(json!({
                "action": "publish_release",
                "actor_id": "alice",
                "risk_tier": "r3",
                "requested_approver": "bob",
                "approval_recipient": "approvals@example.com",
                "approval_channel": "notification",
            }))
            .expect("an approval request"),
            Some(
                serde_json::from_value(json!({"identity": "alice", "mob_id": default_mob}))
                    .expect("an origin"),
            ),
        )
        .await
        .pending_id
        .expect("the action waits for an approval");
    let of_the_default = host_deliveries_of(&runtime, None).await;
    let under_the_id = host_deliveries_of(&runtime, Some(default_mob.as_str())).await;
    let kept: Vec<Value> = runtime
        .list_gating_pending()
        .await
        .iter()
        .map(|entry| serde_json::to_value(entry).expect("a pending request as JSON"))
        .collect();
    let _ = runtime.shutdown().await;

    assert_eq!(
        approvals_notified(&of_the_default),
        [pending_id],
        "the default mob's deliveries hold the notice: {of_the_default:?}"
    );
    assert_eq!(
        of_the_default
            .iter()
            .map(|record| (record["sink"].clone(), record.get("mob_id").cloned()))
            .collect::<Vec<_>>(),
        [(json!("sms"), None)],
        "the notice went over the default mob's route and names no mob: {of_the_default:?}"
    );
    assert!(
        under_the_id.is_empty(),
        "no delivery is kept under the default mob's id: {under_the_id:?}"
    );
    assert_eq!(
        kept.iter()
            .map(|entry| entry["origin"].get("mob_id").cloned())
            .collect::<Vec<_>>(),
        [None],
        "the request the host keeps names no mob: {kept:?}"
    );
}

/// A host of one mob: a resolution of the default mob has the eight members
/// it always had and no other, it is sent, and the record has the twelve
/// members it always had and no other.
#[test]
fn a_resolution_and_a_delivery_of_the_default_mob_have_the_members_they_always_had() {
    let mut runtime = routing_delivery_runtime();
    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"one-mob-resolve","method":"meta/routing/resolve","params":{"recipient":"approvals@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "one-mob-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    runtime.shutdown();

    let members = |value: &Value| -> Vec<String> {
        let mut names: Vec<String> = value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    };
    assert_eq!(
        members(&resolved["result"]),
        [
            "backoff_ms",
            "channel",
            "rate_limit_per_minute",
            "recipient",
            "retry_max",
            "route_id",
            "sink",
            "target_module",
        ],
        "{resolved}"
    );
    assert_eq!(sent["result"]["status"], json!("sent"), "{sent}");
    assert_eq!(
        members(&sent["result"]),
        [
            "attempts",
            "delivery_id",
            "final_attempt_ms",
            "first_attempt_ms",
            "idempotency_key",
            "payload",
            "recipient",
            "route_id",
            "sink",
            "sink_adapter",
            "status",
            "target_module",
        ],
        "{sent}"
    );
}

#[test]
#[ignore]
fn phase10_choke_107_routing_resolve_hands_off_to_delivery_send() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","channel":"transactional"}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(resolved["result"]["target_module"], json!("delivery"));
    assert_eq!(resolved["result"]["sink"], json!("email"));
    assert_eq!(sent["result"]["route_id"], resolved["result"]["route_id"]);
    assert_eq!(sent["result"]["status"], json!("sent"));
    assert_eq!(
        sent["result"]["attempts"],
        json!([
            {"attempt":1,"status":"transient_failure","backoff_ms":250},
            {"attempt":2,"status":"sent","backoff_ms":0}
        ])
    );
}

#[test]
#[ignore]
fn phase10_e2e_1001_routing_delivery_flow_history_and_rate_limit() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-e2e-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com"}}"#,
        Duration::from_secs(1),
    ));

    let send_a = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-send-a",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-key-a",
                "payload": {"message": "a"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_b = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-send-b",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-key-b",
                "payload": {"message": "b"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_c = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-send-c",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-key-c",
                "payload": {"message": "c"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-history","method":"meta/delivery/history","params":{"recipient":"user@example.com","limit":10}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(send_a["result"]["status"], json!("sent"));
    assert_eq!(send_b["result"]["status"], json!("sent"));
    assert_eq!(send_c["error"]["code"], json!(-32602));
    assert_eq!(
        send_c["error"]["message"],
        json!("Invalid params: rate limit exceeded for sink 'email' at window 0 (limit=2)")
    );
    assert_eq!(
        history["result"]["deliveries"]
            .as_array()
            .map_or(0, Vec::len),
        2
    );
    assert_eq!(
        history["result"]["deliveries"][0]["route_id"],
        resolved["result"]["route_id"]
    );
    assert_eq!(
        history["result"]["deliveries"][1]["route_id"],
        resolved["result"]["route_id"]
    );
}

#[test]
#[ignore]
fn phase10_rpc_invalid_params_for_routing_and_delivery_are_typed() {
    let mut runtime = routing_delivery_runtime();

    let invalid_resolve = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-invalid-resolve","method":"meta/routing/resolve","params":{}}"#,
        Duration::from_secs(1),
    ));
    let invalid_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-invalid-send","method":"meta/delivery/send","params":{"payload":{"message":"x"}}}"#,
        Duration::from_secs(1),
    ));
    let invalid_send_idempotency = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-invalid-send-idempotency","method":"meta/delivery/send","params":{"resolution":{"route_id":"r-1","recipient":"user@example.com","channel":"notification","sink":"email","target_module":"delivery","retry_max":1,"backoff_ms":250,"rate_limit_per_minute":2},"idempotency_key":"","payload":{"message":"x"}}}"#,
        Duration::from_secs(1),
    ));
    let invalid_history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-invalid-history","method":"meta/delivery/history","params":{"limit":0}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(invalid_resolve["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_resolve["error"]["message"],
        json!("Invalid params: recipient must be a non-empty string")
    );

    assert_eq!(invalid_send["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_send["error"]["message"],
        json!("Invalid params: resolution must be an object")
    );

    assert_eq!(invalid_send_idempotency["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_send_idempotency["error"]["message"],
        json!("Invalid params: idempotency_key must be a non-empty string")
    );

    assert_eq!(invalid_history["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_history["error"]["message"],
        json!("Invalid params: limit must be an integer between 1 and 200")
    );
}

#[test]
#[ignore]
fn phase10_forged_delivery_send_resolution_is_rejected() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-forged-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","channel":"transactional"}}"#,
        Duration::from_secs(1),
    ));
    let mut forged_resolution = resolved["result"].clone();
    forged_resolution["sink"] = json!("sms");
    let forged_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-forged-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": forged_resolution,
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(forged_send["error"]["code"], json!(-32602));
    assert_eq!(
        forged_send["error"]["message"],
        json!("Invalid params: resolution does not match the trusted route for route_id")
    );
}

#[test]
#[ignore]
fn phase10_retry_max_greater_than_one_is_honored_deterministically() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-retry-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":3,"backoff_ms":125}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-retry-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(sent["result"]["status"], json!("sent"));
    assert_eq!(
        sent["result"]["attempts"],
        json!([
            {"attempt":1,"status":"transient_failure","backoff_ms":125},
            {"attempt":2,"status":"transient_failure","backoff_ms":125},
            {"attempt":3,"status":"transient_failure","backoff_ms":125},
            {"attempt":4,"status":"sent","backoff_ms":0}
        ])
    );
    assert_eq!(
        sent["result"]["final_attempt_ms"]
            .as_u64()
            .unwrap_or_default()
            - sent["result"]["first_attempt_ms"]
                .as_u64()
                .unwrap_or_default(),
        375
    );
}

#[test]
#[ignore]
fn phase10_idempotency_payload_mismatch_and_post_eviction_replay_are_correct() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-idem-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));
    let first_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-idem-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-shared-key",
                "payload": {"message": "hello-a"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let mismatch_replay = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-idem-send-mismatch",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-shared-key",
                "payload": {"message": "hello-b"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    for idx in 0..201 {
        let _ = parse_response(&handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("phase10-idem-fill-{idx}"),
                "method": "meta/delivery/send",
                "params": {
                    "resolution": resolved["result"].clone(),
                    "idempotency_key": format!("phase10-fill-key-{idx}"),
                    "payload": {"message": format!("fill-{idx}")}
                }
            })
            .to_string(),
            Duration::from_secs(1),
        ));
    }

    let replay_after_eviction = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-idem-send-after-evict",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-shared-key",
                "payload": {"message": "hello-a"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-idem-history","method":"meta/delivery/history","params":{"recipient":"user@example.com","limit":200}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(mismatch_replay["error"]["code"], json!(-32602));
    assert_eq!(
        mismatch_replay["error"]["message"],
        json!("Invalid params: idempotency_key replay payload does not match original request")
    );
    assert_eq!(replay_after_eviction["result"]["status"], json!("sent"));
    assert_ne!(
        replay_after_eviction["result"]["delivery_id"],
        first_send["result"]["delivery_id"]
    );
    assert_eq!(
        history["result"]["deliveries"]
            .as_array()
            .map_or(0, Vec::len),
        200
    );
}

#[test]
#[ignore]
fn phase10_retry_max_above_cap_is_rejected() {
    let mut runtime = routing_delivery_runtime();

    let rejected = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-retry-cap","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":11}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(rejected["error"]["code"], json!(-32602));
    assert_eq!(
        rejected["error"]["message"],
        json!("Invalid params: retry_max must be <= 10")
    );
}

#[test]
#[ignore]
fn phase10_retry_max_and_rate_limit_validation_values_are_rejected() {
    let mut runtime = routing_delivery_runtime();

    let retry_overflow = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-retry-overflow",
            "method": "meta/routing/resolve",
            "params": {
                "recipient": "user@example.com",
                "retry_max": 4_294_967_296_u64
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let rate_limit_overflow = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-overflow",
            "method": "meta/routing/resolve",
            "params": {
                "recipient": "user@example.com",
                "rate_limit_per_minute": 4_294_967_296_u64
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let rate_limit_zero = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-zero",
            "method": "meta/routing/resolve",
            "params": {
                "recipient": "user@example.com",
                "rate_limit_per_minute": 0
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(retry_overflow["error"]["code"], json!(-32602));
    assert_eq!(
        retry_overflow["error"]["message"],
        json!("Invalid params: retry_max exceeds maximum supported integer range")
    );
    assert_eq!(rate_limit_overflow["error"]["code"], json!(-32602));
    assert_eq!(
        rate_limit_overflow["error"]["message"],
        json!("Invalid params: rate_limit_per_minute exceeds maximum supported integer range")
    );
    assert_eq!(rate_limit_zero["error"]["code"], json!(-32602));
    assert_eq!(
        rate_limit_zero["error"]["message"],
        json!("Invalid params: rate_limit_per_minute must be greater than 0")
    );
}

#[test]
#[ignore]
fn phase10_rate_window_counts_remain_single_window_for_rapid_sends() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-prune-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));

    for idx in 0..121 {
        let sent = parse_response(&handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("phase10-prune-send-{idx}"),
                "method": "meta/delivery/send",
                "params": {
                    "resolution": resolved["result"].clone(),
                    "idempotency_key": format!("phase10-prune-key-{idx}"),
                    "payload": {"message": format!("prune-{idx}")}
                }
            })
            .to_string(),
            Duration::from_secs(1),
        ));
        assert_eq!(sent["result"]["status"], json!("sent"));
    }

    assert_eq!(runtime.delivery_rate_window_count_entries(), 1);
    runtime.shutdown();
}

#[test]
#[ignore]
fn phase10_rate_limit_is_isolated_per_route_scope() {
    let mut runtime = routing_delivery_runtime();

    let route_a = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-a","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":2}}"#,
        Duration::from_secs(1),
    ));
    let send_a_1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-a-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-route-a-key-1",
                "payload": {"message": "a1"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_a_2 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-a-send-2",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-route-a-key-2",
                "payload": {"message": "a2"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_a_3 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-a-send-3",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-route-a-key-3",
                "payload": {"message": "a3"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    let route_b = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-b","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":2}}"#,
        Duration::from_secs(1),
    ));
    let send_b_1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-b-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_b["result"].clone(),
                "idempotency_key": "phase10-route-b-key-1",
                "payload": {"message": "b1"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(send_a_1["result"]["status"], json!("sent"));
    assert_eq!(send_a_2["result"]["status"], json!("sent"));
    assert_eq!(send_a_3["error"]["code"], json!(-32602));
    assert_eq!(
        send_a_3["error"]["message"],
        json!("Invalid params: rate limit exceeded for sink 'email' at window 0 (limit=2)")
    );
    assert_eq!(send_b_1["result"]["status"], json!("sent"));
}

#[test]
#[ignore]
fn phase10_rejected_rate_limited_calls_do_not_advance_delivery_clock() {
    let mut runtime = routing_delivery_runtime();

    let route_a = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-clock-rate-route-a","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1}}"#,
        Duration::from_secs(1),
    ));
    let first_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-clock-rate-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-clock-rate-key-1",
                "payload": {"message": "first"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    for idx in 0..5 {
        let rejected = parse_response(&handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("phase10-clock-rate-reject-{idx}"),
                "method": "meta/delivery/send",
                "params": {
                    "resolution": route_a["result"].clone(),
                    "idempotency_key": format!("phase10-clock-rate-reject-key-{idx}"),
                    "payload": {"message": format!("reject-{idx}")}
                }
            })
            .to_string(),
            Duration::from_secs(1),
        ));
        assert_eq!(rejected["error"]["code"], json!(-32602));
        assert_eq!(
            rejected["error"]["message"],
            json!("Invalid params: rate limit exceeded for sink 'email' at window 0 (limit=1)")
        );
    }

    let route_b = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-clock-rate-route-b","method":"meta/routing/resolve","params":{"recipient":"other@example.com","retry_max":0,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));
    let second_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-clock-rate-send-2",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_b["result"].clone(),
                "idempotency_key": "phase10-clock-rate-key-2",
                "payload": {"message": "second"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(first_send["result"]["status"], json!("sent"));
    assert_eq!(second_send["result"]["status"], json!("sent"));
    assert_eq!(
        second_send["result"]["first_attempt_ms"]
            .as_u64()
            .unwrap_or_default(),
        first_send["result"]["first_attempt_ms"]
            .as_u64()
            .unwrap_or_default()
            .saturating_add(1_000)
    );
}

#[test]
#[ignore]
fn phase10_rate_limit_cannot_be_bypassed_via_artificial_delivery_clock_advancement() {
    let mut runtime = routing_delivery_runtime();

    let route_a = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-rate-now-route-a","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1}}"#,
        Duration::from_secs(1),
    ));
    let send_a_1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-now-send-a-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-rate-now-key-a-1",
                "payload": {"message": "a1"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    let route_b = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-rate-now-route-b","method":"meta/routing/resolve","params":{"recipient":"other@example.com","retry_max":10,"backoff_ms":60000,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));
    let send_b_1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-now-send-b-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_b["result"].clone(),
                "idempotency_key": "phase10-rate-now-key-b-1",
                "payload": {"message": "b1"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    let send_a_2 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-now-send-a-2",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-rate-now-key-a-2",
                "payload": {"message": "a2"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(send_a_1["result"]["status"], json!("sent"));
    assert_eq!(send_b_1["result"]["status"], json!("sent"));
    assert_eq!(send_a_2["error"]["code"], json!(-32602));
    assert!(
        send_a_2["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("rate limit exceeded for sink 'email'"))
    );
}

#[test]
#[ignore]
fn phase10_routing_resolved_timestamp_never_regresses_merged_timeline() {
    let mut runtime = routing_delivery_runtime();
    let baseline_max_timestamp_ms = runtime
        .merged_events()
        .iter()
        .map(|envelope| envelope.timestamp_ms)
        .max()
        .unwrap_or_default();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-resolved-ts-floor","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":2}}"#,
        Duration::from_secs(1),
    ));
    let route_id = resolved["result"]["route_id"]
        .as_str()
        .expect("resolved route id");
    let resolved_timestamp_ms = runtime
        .merged_events()
        .iter()
        .find_map(|envelope| match &envelope.event {
            UnifiedEvent::Module(event)
                if event.module == "router"
                    && event.event_type == "resolved"
                    && event
                        .payload
                        .get("route_id")
                        .and_then(Value::as_str)
                        .is_some_and(|candidate| candidate == route_id) =>
            {
                Some(envelope.timestamp_ms)
            }
            _ => None,
        })
        .expect("resolve event timestamp");

    runtime.shutdown();

    assert!(resolved_timestamp_ms >= baseline_max_timestamp_ms);
}

#[test]
#[ignore]
fn phase10_delivery_final_attempt_preserves_monotonic_event_timestamps() {
    let mut runtime = routing_delivery_runtime();

    let first_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-monotonic-resolve-1","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":3,"backoff_ms":125}}"#,
        Duration::from_secs(1),
    ));
    let first_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-monotonic-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": first_route["result"].clone(),
                "idempotency_key": "phase10-monotonic-key-1",
                "payload": {"message": "first"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let second_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-monotonic-resolve-2","method":"meta/routing/resolve","params":{"recipient":"another@example.com","retry_max":0}}"#,
        Duration::from_secs(1),
    ));

    let second_route_id = second_route["result"]["route_id"]
        .as_str()
        .expect("second route id");
    let second_resolve_ts = runtime
        .merged_events()
        .iter()
        .find_map(|envelope| match &envelope.event {
            UnifiedEvent::Module(event)
                if event.module == "router"
                    && event.event_type == "resolved"
                    && event
                        .payload
                        .get("route_id")
                        .and_then(Value::as_str)
                        .is_some_and(|route_id| route_id == second_route_id) =>
            {
                Some(envelope.timestamp_ms)
            }
            _ => None,
        })
        .expect("find router resolved event for second route");

    runtime.shutdown();

    assert_eq!(first_send["result"]["status"], json!("sent"));
    assert!(
        second_resolve_ts
            >= first_send["result"]["final_attempt_ms"]
                .as_u64()
                .unwrap_or_default()
    );
}

#[test]
#[ignore]
fn phase10_idempotent_replay_survives_routing_resolution_cache_eviction() {
    let mut runtime = routing_delivery_runtime();

    let original_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-evict-resolve-0","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));
    let first_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-evict-send-0",
            "method": "meta/delivery/send",
            "params": {
                "resolution": original_route["result"].clone(),
                "idempotency_key": "phase10-route-evict-key",
                "payload": {"message": "same"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    for idx in 0..513 {
        let _ = parse_response(&handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("phase10-route-evict-resolve-fill-{idx}"),
                "method": "meta/routing/resolve",
                "params": {
                    "recipient": format!("evict-{idx}@example.com"),
                    "retry_max": 0,
                    "rate_limit_per_minute": 1000
                }
            })
            .to_string(),
            Duration::from_secs(1),
        ));
    }
    let mut forged_resolution_after_eviction = original_route["result"].clone();
    forged_resolution_after_eviction["sink"] = json!("sms");

    let non_replay_after_eviction = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-evict-send-non-replay",
            "method": "meta/delivery/send",
            "params": {
                "resolution": original_route["result"].clone(),
                "idempotency_key": "phase10-route-evict-new-key",
                "payload": {"message": "new"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let forged_replay_after_eviction = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-evict-send-forged-replay",
            "method": "meta/delivery/send",
            "params": {
                "resolution": forged_resolution_after_eviction,
                "idempotency_key": "phase10-route-evict-key",
                "payload": {"message": "same"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let replay_after_eviction = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-evict-send-replay",
            "method": "meta/delivery/send",
            "params": {
                "resolution": original_route["result"].clone(),
                "idempotency_key": "phase10-route-evict-key",
                "payload": {"message": "same"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(non_replay_after_eviction["error"]["code"], json!(-32602));
    assert_eq!(
        non_replay_after_eviction["error"]["message"],
        json!(
            "Invalid params: resolution.route_id 'route-000000' was not issued by routing/resolve"
        )
    );
    assert_eq!(forged_replay_after_eviction["error"]["code"], json!(-32602));
    assert_eq!(
        forged_replay_after_eviction["error"]["message"],
        json!("Invalid params: resolution does not match the trusted route for route_id")
    );
    assert_eq!(
        replay_after_eviction["result"]["delivery_id"],
        first_send["result"]["delivery_id"]
    );
}

#[test]
#[ignore]
fn phase10_routing_and_delivery_paths_consume_module_boundary_outputs() {
    let mut runtime = routing_delivery_runtime_with_env(
        &[("META_PHASE_C_ROUTER_SINK", "webhook")],
        &[("META_PHASE_C_DELIVERY_ADAPTER", "smtp-mock")],
    );

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-boundary-resolve","method":"meta/routing/resolve","params":{"recipient":"boundary@example.com","channel":"transactional"}}"#,
        Duration::from_secs(1),
    ));
    let sent_ok = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-boundary-send-ok",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "ok"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let sent_failed = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-boundary-send-fail",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "bad", "force_adapter_fail": true}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("webhook"));
    assert_eq!(sent_ok["result"]["status"], json!("sent"));
    assert_eq!(sent_ok["result"]["sink_adapter"], json!("smtp-mock"));
    assert_eq!(sent_failed["result"]["status"], json!("failed"));
    assert_eq!(
        sent_failed["result"]["attempts"][1]["status"],
        json!("failed")
    );
}

#[test]
#[ignore]
fn phase10_runtime_route_update_surface_changes_resolve_behavior() {
    let mut runtime = routing_delivery_runtime();

    let list_initial = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-list-initial","method":"meta/routing/routes/list","params":{}}"#,
        Duration::from_secs(1),
    ));
    let add_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-add","method":"meta/routing/routes/add","params":{"route":{"route_key":"vip-route","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery","retry_max":0,"backoff_ms":5,"rate_limit_per_minute":9}}}"#,
        Duration::from_secs(1),
    ));
    let list_after_add = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-list-after-add","method":"meta/routing/routes/list","params":{}}"#,
        Duration::from_secs(1),
    ));
    let resolved_with_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-resolve-with-route","method":"meta/routing/resolve","params":{"recipient":"vip@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let deleted_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-delete","method":"meta/routing/routes/delete","params":{"route_key":"vip-route"}}"#,
        Duration::from_secs(1),
    ));
    let resolved_without_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-resolve-without-route","method":"meta/routing/resolve","params":{"recipient":"vip@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let invalid_add = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-add-invalid","method":"meta/routing/routes/add","params":{"route":{"recipient":"oops@example.com","sink":"email","target_module":"delivery"}}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(list_initial["result"]["routes"], json!([]));
    assert_eq!(
        add_route["result"]["route"]["route_key"],
        json!("vip-route")
    );
    assert_eq!(
        list_after_add["result"]["routes"]
            .as_array()
            .map_or(0, Vec::len),
        1
    );
    assert_eq!(resolved_with_route["result"]["sink"], json!("sms"));
    assert_eq!(resolved_with_route["result"]["retry_max"], json!(0));
    assert_eq!(resolved_with_route["result"]["backoff_ms"], json!(5));
    assert_eq!(
        resolved_with_route["result"]["rate_limit_per_minute"],
        json!(9)
    );
    assert_eq!(
        deleted_route["result"]["deleted"]["route_key"],
        json!("vip-route")
    );
    assert_eq!(resolved_without_route["result"]["sink"], json!("email"));
    assert_eq!(invalid_add["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_add["error"]["message"],
        json!("Invalid params: route_key must be a non-empty string")
    );
}
