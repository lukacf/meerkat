//! Rows of the identity delivery methods over stdio on a host with two
//! mobs: a scoped `meta/dispatch` read back with `meta/recover_delivery`,
//! and a keyed `meta/send` read back with `meta/send_status`.
//!
//! The fixture is the launch of the mob entries' rows: a host launched for
//! real, a second mob built by the host's mob set, an identity context for
//! each mob, and an agent `desk` in each. Every request goes through the
//! stdio dispatch, which is handed the launch's identity context as a
//! gateway hands it; a request for the second mob names it in `mob_id`.

#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::MobId;
use serde_json::{Value, json};

use super::UnifiedRuntime;
use super::mob_entries_tests::{
    Sessions, configured_agent, definition, identity_context, launch, launch_keeping, second_mob,
    set_of, shut_down, transcript, transcript_showing,
};

/// How often a row reads a committed state again before it judges it: two
/// hundred reads, a tenth of a second apart.
const READS: usize = 200;

/// A host with two mobs, an agent `desk` in each.
struct TwoMobs {
    host: UnifiedRuntime,
    sessions: Sessions,
    second_id: MobId,
    /// The identity context the launch hands the stdio dispatch.
    of_the_launch: crate::rpc::IdentityFirstContext,
}

/// The identity context a gateway hands the stdio dispatch for its launch
/// mob: the one the launch installed.
fn context_of_the_launch(host: &UnifiedRuntime) -> crate::rpc::IdentityFirstContext {
    let context = host
        .identity_first_context()
        .expect("the launch installed an identity context");
    crate::rpc::IdentityFirstContext {
        runtime: context.runtime(),
        roster_provider: Arc::clone(&context.roster_provider),
        topology_provider: context.topology_provider.clone(),
        customizer: context.customizer.clone(),
        agent_memory_provider: None,
        mob_definition: Some(host.mob_handle().definition().clone()),
        transcript_edit_service: None,
        compaction_floors: None,
    }
}

async fn two_mobs_each_with_a_desk(temp: &tempfile::TempDir) -> TwoMobs {
    let (mut host, sessions) = launch(temp, definition("host")).await;
    let second_id = second_mob(&host, definition("second")).await;
    let of_the_default = identity_context(host.mob_runtime(), "default");
    Box::pin(host.install_and_bootstrap_identity_first_context(
        Arc::clone(&of_the_default),
        &[configured_agent("desk")],
    ))
    .await
    .expect("the default mob's identity bootstrap");
    {
        let mob = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        let of_the_second = identity_context(mob.mob_runtime(), "second");
        mob.attach_identity_first_context(Arc::clone(&of_the_second))
            .await
            .expect("a host attaches an identity context to its second mob");
        Box::pin(of_the_second.bootstrap_roster(&[configured_agent("desk")]))
            .await
            .expect("the second mob's roster");
    }
    let of_the_launch = context_of_the_launch(&host);
    TwoMobs {
        host,
        sessions,
        second_id,
        of_the_launch,
    }
}

impl TwoMobs {
    /// One request through the stdio dispatch, for the mob `mob` names (the
    /// launch mob when it names none); the whole reply.
    async fn call(&self, mob: Option<&MobId>, method: &str, mut params: Value) -> Value {
        if let (Some(mob), Some(members)) = (mob, params.as_object_mut()) {
            members.insert("mob_id".to_string(), json!(mob.as_str()));
        }
        let request =
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        let reply = crate::rpc::handle_unified_rpc_json(
            &self.host,
            &request,
            Duration::from_secs(30),
            None,
            Some(&self.of_the_launch),
        )
        .await;
        serde_json::from_str(&reply).expect("the dispatch answers JSON")
    }

    /// [`Self::call`] for a request that must be answered; its result.
    async fn rpc(&self, mob: Option<&MobId>, method: &str, params: Value) -> Value {
        let reply = self.call(mob, method, params).await;
        assert!(
            reply.get("error").is_none(),
            "{method} for {mob:?} is answered: {reply}"
        );
        reply["result"].clone()
    }

    /// The delivery scope `meta/status_identity` names for the mob's `desk`.
    async fn scope_of_the_desk(&self, mob: Option<&MobId>) -> Value {
        let status = self
            .rpc(mob, "meta/status_identity", json!({"identity": "desk"}))
            .await;
        assert!(
            status["delivery_scope"].is_object(),
            "the desk of {mob:?} has a delivery scope: {status}"
        );
        status["delivery_scope"].clone()
    }

    /// A scoped `meta/dispatch` of `text` to the mob's `desk`; the whole
    /// reply.
    async fn scoped_dispatch(
        &self,
        mob: Option<&MobId>,
        scope: &Value,
        text: &str,
        key: &str,
        correlation: &str,
    ) -> Value {
        self.call(
            mob,
            "meta/dispatch",
            json!({
                "identity": "desk",
                "dispatch_input": {
                    "content": text,
                    "idempotency_key": key,
                    "correlation_id": correlation,
                },
                "expected_scope": scope,
            }),
        )
        .await
    }

    /// One `meta/recover_delivery` of the mob's `desk`; the whole reply.
    async fn recover(
        &self,
        mob: Option<&MobId>,
        scope: &Value,
        key: &str,
        correlation: &str,
    ) -> Value {
        self.call(
            mob,
            "meta/recover_delivery",
            json!({
                "identity": "desk",
                "scope": scope,
                "idempotency_key": key,
                "correlation_id": correlation,
                "timeout_ms": 500,
            }),
        )
        .await
    }

    /// [`Self::recover`], read again until the delivery is `completed` or
    /// the reads are used up; the last reply.
    async fn recovered(
        &self,
        mob: Option<&MobId>,
        scope: &Value,
        key: &str,
        correlation: &str,
    ) -> Value {
        let mut reply = self.recover(mob, scope, key, correlation).await;
        for _ in 0..READS {
            if reply["result"]["recovery"]["state"] == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            reply = self.recover(mob, scope, key, correlation).await;
        }
        reply
    }

    /// A keyed `meta/send` of `text` to the mob's `desk`; its result.
    async fn keyed_send(&self, mob: Option<&MobId>, text: &str, key: &str) -> Value {
        let result = self
            .rpc(
                mob,
                "meta/send",
                json!({"identity": "desk", "content": text, "idempotency_key": key}),
            )
            .await;
        assert!(
            result["continuation"]["delivery_id"].is_string(),
            "a keyed send answers its continuation: {result}"
        );
        result
    }

    /// [`Self::keyed_send`] under the same key, sent again until its status
    /// is `admitted` or the reads are used up; the last result.
    async fn admitted(&self, mob: Option<&MobId>, text: &str, key: &str) -> Value {
        let mut result = self.keyed_send(mob, text, key).await;
        for _ in 0..READS {
            if result["continuation"]["status"]["state"] == "admitted" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            result = self.keyed_send(mob, text, key).await;
        }
        assert_eq!(
            result["continuation"]["status"]["state"], "admitted",
            "the keyed send to the desk of {mob:?} is admitted: {result}"
        );
        result
    }

    /// One `meta/send_status` of the mob's `desk` for the admitted pair
    /// `status` names; the whole reply.
    async fn send_status(&self, mob: Option<&MobId>, status: &Value) -> Value {
        self.call(
            mob,
            "meta/send_status",
            json!({
                "identity": "desk",
                "session_id": status["session_id"],
                "input_id": status["input_id"],
            }),
        )
        .await
    }

    /// [`Self::send_status`], read again until the input is terminal or the
    /// reads are used up; the last reply.
    async fn terminal_status(&self, mob: Option<&MobId>, status: &Value) -> Value {
        let mut reply = self.send_status(mob, status).await;
        for _ in 0..READS {
            if reply["result"]["state"]["terminal_outcome"].is_string() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            reply = self.send_status(mob, status).await;
        }
        reply
    }

    /// The sessions of the two members named `desk`: the default mob's,
    /// then the second mob's.
    async fn sessions_of_the_desks(
        &self,
    ) -> (
        Option<meerkat_core::types::SessionId>,
        Option<meerkat_core::types::SessionId>,
    ) {
        let desk = meerkat_mob::AgentIdentity::from("desk");
        let second = set_of(&self.host)
            .handle_for(&self.second_id)
            .await
            .expect("the set serves the second mob");
        (
            self.host
                .mob_handle()
                .resolve_bridge_session_id(&desk)
                .await,
            second.resolve_bridge_session_id(&desk).await,
        )
    }

    /// Each mob's `desk` holds its own word and not the other mob's.
    async fn each_desk_holds_its_own_word_alone(&self, of_the_default: &str, of_the_second: &str) {
        let (in_the_default, in_the_second) = self.sessions_of_the_desks().await;
        assert!(
            in_the_default.is_some() && in_the_second.is_some() && in_the_default != in_the_second,
            "two members, one name: {in_the_default:?}, {in_the_second:?}"
        );
        let (Some(in_the_default), Some(in_the_second)) = (in_the_default, in_the_second) else {
            return;
        };
        // Each member's own word first, so that both turns have begun before
        // either transcript is judged for the other mob's word.
        transcript_showing(&self.sessions, &in_the_default, of_the_default).await;
        transcript_showing(&self.sessions, &in_the_second, of_the_second).await;
        let (default_desk, second_desk) = (
            transcript(&self.sessions, &in_the_default).await,
            transcript(&self.sessions, &in_the_second).await,
        );
        assert!(
            default_desk.contains(of_the_default) && !default_desk.contains(of_the_second),
            "the default mob's desk holds its own word alone: {default_desk}"
        );
        assert!(
            second_desk.contains(of_the_second) && !second_desk.contains(of_the_default),
            "the second mob's desk holds its own word alone: {second_desk}"
        );
    }

    async fn shut_down(self) {
        shut_down(&self.host, &[&self.second_id]).await;
    }
}

/// One key and one correlation, dispatched with a scope into the `desk` of
/// each mob of a host: each mob accepts the dispatch for its own scope,
/// each delivery is recovered as completed by the mob it was made in, and
/// the launch mob, asked for the second mob's scope, proves no binding and
/// reads nothing of it. Each word is in its own mob's member alone.
#[tokio::test]
async fn a_scoped_dispatch_for_a_hosted_mob_is_recovered_by_that_mob_alone() {
    const TO_THE_DEFAULT: &str = "a scoped word for the desk of the default mob";
    const TO_THE_SECOND: &str = "a scoped word for the desk of the second mob";
    const KEY: &str = "one-occurrence";
    let temp = tempfile::tempdir().expect("tempdir");
    let mobs = two_mobs_each_with_a_desk(&temp).await;
    let second = mobs.second_id.clone();
    let correlation = uuid::Uuid::new_v4().to_string();

    let scope_in_the_default = mobs.scope_of_the_desk(None).await;
    let scope_in_the_second = mobs.scope_of_the_desk(Some(&second)).await;
    assert_ne!(
        scope_in_the_default, scope_in_the_second,
        "the desk of each mob has a delivery scope of its own"
    );

    for (mob, scope, word) in [
        (None, &scope_in_the_default, TO_THE_DEFAULT),
        (Some(&second), &scope_in_the_second, TO_THE_SECOND),
    ] {
        let dispatched = mobs
            .scoped_dispatch(mob, scope, word, KEY, &correlation)
            .await;
        assert!(
            dispatched.get("error").is_none(),
            "the scoped dispatch for {mob:?} is accepted: {dispatched}"
        );
        assert_eq!(
            dispatched["result"]["receipt"]["stage"], "ingress_accepted",
            "{dispatched}"
        );
        assert_eq!(
            dispatched["result"]["delivery_scope"], *scope,
            "the dispatch for {mob:?} answers the scope it was made at: {dispatched}"
        );
    }

    for (mob, scope) in [
        (None, &scope_in_the_default),
        (Some(&second), &scope_in_the_second),
    ] {
        let recovered = mobs.recovered(mob, scope, KEY, &correlation).await;
        assert!(recovered.get("error").is_none(), "{recovered}");
        assert_eq!(
            recovered["result"]["recovery"]["state"], "completed",
            "the delivery made for {mob:?} is recovered there: {recovered}"
        );
    }

    let asked_of_the_launch = mobs
        .recover(None, &scope_in_the_second, KEY, &correlation)
        .await;
    assert!(
        asked_of_the_launch.get("error").is_none(),
        "{asked_of_the_launch}"
    );
    assert_eq!(
        asked_of_the_launch["result"]["recovery"]["state"], "unresolved",
        "the launch mob reads no delivery of the second mob's scope: {asked_of_the_launch}"
    );
    assert_eq!(
        asked_of_the_launch["result"]["recovery"]["cause"], "scope_binding_unproven",
        "{asked_of_the_launch}"
    );

    mobs.each_desk_holds_its_own_word_alone(TO_THE_DEFAULT, TO_THE_SECOND)
        .await;
    mobs.shut_down().await;
}

/// One send key, used for a keyed `meta/send` to the `desk` of each mob of
/// a host: two sends, each admitted in its own mob's member under an input
/// of its own, and `meta/send_status` of each mob follows its own input to
/// its end. A send again under the key in one mob is that mob's delivery
/// again, and the launch mob refuses to read the session of the second
/// mob's member. Each word is in its own mob's member alone.
#[tokio::test]
async fn one_send_key_in_two_mobs_is_two_sends_each_with_its_own_status() {
    const TO_THE_DEFAULT: &str = "a keyed word for the desk of the default mob";
    const TO_THE_SECOND: &str = "a keyed word for the desk of the second mob";
    const KEY: &str = "one-key";
    let temp = tempfile::tempdir().expect("tempdir");
    let mobs = two_mobs_each_with_a_desk(&temp).await;
    let second = mobs.second_id.clone();

    let in_the_default = mobs.admitted(None, TO_THE_DEFAULT, KEY).await;
    let in_the_second = mobs.admitted(Some(&second), TO_THE_SECOND, KEY).await;
    let (status_in_the_default, status_in_the_second) = (
        in_the_default["continuation"]["status"].clone(),
        in_the_second["continuation"]["status"].clone(),
    );
    assert_ne!(
        in_the_default["continuation"]["delivery_id"], in_the_second["continuation"]["delivery_id"],
        "one key in two mobs is two deliveries"
    );
    assert_ne!(
        status_in_the_default["session_id"], status_in_the_second["session_id"],
        "each send is admitted in its own mob's member"
    );
    assert_ne!(
        status_in_the_default["input_id"], status_in_the_second["input_id"],
        "each send is an input of its own"
    );

    for (mob, status) in [
        (None, &status_in_the_default),
        (Some(&second), &status_in_the_second),
    ] {
        let terminal = mobs.terminal_status(mob, status).await;
        assert!(terminal.get("error").is_none(), "{terminal}");
        assert_eq!(
            terminal["result"]["state"]["input_id"], status["input_id"],
            "the status read for {mob:?} follows its own input: {terminal}"
        );
        assert_eq!(
            terminal["result"]["state"]["terminal_outcome"], "completed",
            "{terminal}"
        );
    }

    let again = mobs.keyed_send(Some(&second), TO_THE_SECOND, KEY).await;
    assert_eq!(
        again["continuation"]["delivery_id"], in_the_second["continuation"]["delivery_id"],
        "a send again under the key in the second mob is that mob's delivery: {again}"
    );

    let asked_of_the_launch = mobs.send_status(None, &status_in_the_second).await;
    assert_eq!(
        asked_of_the_launch["error"]["data"]["kind"], "session_not_held_by_identity",
        "the launch mob reads no session of the second mob's member: {asked_of_the_launch}"
    );

    mobs.each_desk_holds_its_own_word_alone(TO_THE_DEFAULT, TO_THE_SECOND)
        .await;
    mobs.shut_down().await;
}

/// The route `kept-route` of the mob `mob_id` names (none for the default
/// mob), from its wire form.
fn kept_route(mob_id: Option<&str>) -> crate::RuntimeRoute {
    let mut route = json!({
        "route_key": "kept-route",
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

/// A delivery the host made for the mob `mob_id` names (none for the
/// default mob), from its wire form.
fn kept_delivery(delivery_id: &str, mob_id: Option<&str>) -> crate::runtime::DeliveryRecord {
    let mut record = json!({
        "delivery_id": delivery_id,
        "route_id": "route-1",
        "recipient": "vip@example.com",
        "sink": "sms",
        "target_module": "delivery",
        "payload": { "text": "hello" },
        "status": "delivered",
        "attempts": [],
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

/// What the host's delivery service keeps for the mob `mob_id` names (none
/// for the default mob): whether it holds the route `kept-route`, and the
/// ids of its deliveries.
async fn kept_for(host: &UnifiedRuntime, mob_id: Option<&str>) -> (bool, Vec<String>) {
    let module_runtime = host.module_runtime_handle();
    let module_runtime = module_runtime.lock().await;
    let has_the_route = module_runtime.list_runtime_routes().iter().any(|route| {
        route.route_key == "kept-route" && route.mob_id.as_ref().map(MobId::as_str) == mob_id
    });
    let mut request = json!({ "limit": 50 });
    if let Some(mob_id) = mob_id {
        request["mob_id"] = json!(mob_id);
    }
    let deliveries = module_runtime
        .delivery_history(serde_json::from_value(request).expect("a history request"))
        .deliveries
        .iter()
        .map(|record| record.delivery_id.clone())
        .collect();
    (has_the_route, deliveries)
}

/// A mob the host creates under an id starts with nothing the host's
/// delivery service kept under that id. A route and a delivery are left
/// under an id no mob has, as a destroyed mob leaves them when what the
/// host kept of it was not forgotten at its destroy. The mob created under
/// that id has no route and no delivery, and the default mob keeps its
/// own.
#[tokio::test]
async fn a_mob_created_under_an_id_has_none_of_the_routes_and_deliveries_kept_under_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let of_the_second = definition("second");
    let second_id = of_the_second.id.clone();
    {
        let module_runtime = host.module_runtime_handle();
        let mut module_runtime = module_runtime.lock().await;
        for mob_id in [None, Some(second_id.as_str())] {
            module_runtime
                .add_runtime_route(kept_route(mob_id))
                .expect("the route is added");
        }
        module_runtime.record_delivery_for_test(kept_delivery("delivery-of-the-default-mob", None));
        module_runtime.record_delivery_for_test(kept_delivery(
            "delivery-of-an-earlier-mob",
            Some(second_id.as_str()),
        ));
    }
    let left_under_the_id = kept_for(&host, Some(second_id.as_str())).await;

    let created = second_mob(&host, of_the_second).await;
    let of_the_created_mob = kept_for(&host, Some(second_id.as_str())).await;
    let of_the_default_mob = kept_for(&host, None).await;
    shut_down(&host, &[&second_id]).await;

    assert_eq!(created, second_id, "the mob is created under the id");
    assert_eq!(
        left_under_the_id,
        (true, vec!["delivery-of-an-earlier-mob".to_string()]),
        "a route and a delivery were kept under the id before the create"
    );
    assert_eq!(
        of_the_created_mob,
        (false, Vec::new()),
        "the created mob has none of the routes and deliveries kept under its id"
    );
    assert_eq!(
        of_the_default_mob,
        (true, vec!["delivery-of-the-default-mob".to_string()]),
        "the default mob keeps its route and its delivery"
    );
}

/// An approval asked at the host's action boundary for the agent `alice` of
/// the mob `mob_id` names (none for the default mob), with no notice. The
/// pending id of the request.
async fn approval_asked_for(host: &UnifiedRuntime, mob_id: Option<&str>) -> String {
    let mut origin = json!({ "identity": "alice" });
    if let Some(mob_id) = mob_id {
        origin["mob_id"] = json!(mob_id);
    }
    host.evaluate_gating_action_with_origin(
        serde_json::from_value(json!({
            "action": "publish_report",
            "actor_id": "alice",
            "risk_tier": "r3",
            "approval_timeout_ms": 600_000,
        }))
        .expect("a gating request"),
        Some(serde_json::from_value(origin).expect("an origin")),
    )
    .await
    .pending_id
    .expect("the action waits for an approval")
}

/// The pending ids of the approvals the host holds, in the order they were
/// asked.
async fn approvals_waiting(host: &UnifiedRuntime) -> Vec<String> {
    host.list_gating_pending()
        .await
        .into_iter()
        .map(|entry| entry.pending_id)
        .collect()
}

/// A mob the host creates under an id is asked for none of the approvals
/// kept under that id. An approval is left waiting for an agent of an id no
/// mob has, as a destroyed mob leaves it when what the host kept of it was
/// not forgotten at its destroy, and one waits for an agent of the default
/// mob. After the create of a mob under that id the first no longer waits:
/// a decision of it is refused as one of a request that is not pending, and
/// the host's audit says the host withdrew it. The default mob's approval
/// still waits.
#[tokio::test]
async fn a_mob_created_under_an_id_is_asked_for_none_of_the_approvals_kept_under_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    let of_the_second = definition("second");
    let second_id = of_the_second.id.clone();
    let of_the_default_mob = approval_asked_for(&host, None).await;
    let of_an_earlier_mob = approval_asked_for(&host, Some(second_id.as_str())).await;
    let waiting_before = approvals_waiting(&host).await;

    let created = second_mob(&host, of_the_second).await;
    let waiting_after = approvals_waiting(&host).await;
    let decided = host
        .decide_gating_action(
            serde_json::from_value(json!({
                "pending_id": of_an_earlier_mob,
                "approver_id": "bob",
                "decision": "approve",
            }))
            .expect("a decision"),
        )
        .await;
    let withdrawn_by_the_host: Vec<Option<String>> = host
        .gating_audit_entries(usize::MAX)
        .await
        .into_iter()
        .filter(|entry| {
            entry.event_type == "request_withdrawn" && entry.detail["withdrawn_by"] == json!("host")
        })
        .map(|entry| entry.pending_id)
        .collect();
    shut_down(&host, &[&second_id]).await;

    assert_eq!(created, second_id, "the mob is created under the id");
    assert_eq!(
        waiting_before,
        [of_the_default_mob.clone(), of_an_earlier_mob.clone()],
        "both approvals waited before the create"
    );
    assert_eq!(
        waiting_after,
        [of_the_default_mob],
        "the approval kept under the id no longer waits, and the default mob's does"
    );
    assert_eq!(
        decided.as_ref().err().map(ToString::to_string),
        Some(format!("unknown pending id: {of_an_earlier_mob}")),
        "a decision of it is refused as one of a request that is not pending: {decided:?}"
    );
    assert_eq!(
        withdrawn_by_the_host,
        [Some(of_an_earlier_mob)],
        "the audit says the host withdrew it"
    );
}

/// The pending ids of the approvals in the gating state `metadata` holds,
/// none when it holds no state.
async fn stored_approvals(
    metadata: &Arc<dyn crate::runtime::PersistentMetadataStore>,
) -> Vec<String> {
    metadata
        .realm_document(crate::gating_persistence::GATING_STATE_DOCUMENT)
        .await
        .expect("the store is read")
        .map(|document| {
            serde_json::from_str::<crate::runtime::GatingStateSnapshot>(&document)
                .expect("the stored gating state")
                .pending
                .into_iter()
                .map(|entry| entry.pending_id)
                .collect()
        })
        .unwrap_or_default()
}

/// A create under an id answers only after the approvals it withdrew are
/// written to the gating state the host keeps across restarts. The host
/// keeps that state in a store that keeps it across restarts; an approval
/// waits for an agent of an id no mob has, and the state that holds it is
/// written. While no write of the state can go on, the create of a mob
/// under that id does not answer. Once writes go on it answers, and the
/// state in the store no longer holds the approval: a host started again
/// over that store is not asked for it.
#[tokio::test]
async fn a_create_under_an_id_writes_the_approvals_it_withdrew_before_it_answers() {
    let temp = tempfile::tempdir().expect("tempdir");
    let metadata: Arc<dyn crate::runtime::PersistentMetadataStore> = Arc::new(
        crate::runtime::SqliteMetadataStore::open(temp.path().join("host-records.sqlite"))
            .expect("a store that keeps its documents across restarts"),
    );
    let (host, _sessions, _machine, ()) = launch_keeping(
        &temp,
        definition("host"),
        meerkat::Config::default(),
        Arc::clone(&metadata),
        |_| (),
        |spec| spec,
    )
    .await;
    host.keep_gating_state_in_realm().await;
    let writer = host
        .gating_persistence
        .lock()
        .await
        .as_ref()
        .map(|kept| Arc::clone(&kept.persistence))
        .expect("the host keeps its gating state");
    let of_the_second = definition("second");
    let second_id = of_the_second.id.clone();
    let of_an_earlier_mob = approval_asked_for(&host, Some(second_id.as_str())).await;
    let mut written_with_the_approval = false;
    for _ in 0..READS {
        written_with_the_approval = stored_approvals(&metadata)
            .await
            .contains(&of_an_earlier_mob);
        if written_with_the_approval {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let held = writer.hold_writes_for_test().await;
    let create = second_mob(&host, of_the_second);
    tokio::pin!(create);
    let answered_while_held = tokio::time::timeout(Duration::from_secs(2), &mut create)
        .await
        .ok();
    drop(held);
    let answered_before_the_write = answered_while_held.is_some();
    let created = match answered_while_held {
        Some(created) => created,
        None => create.await,
    };
    let stored_after = stored_approvals(&metadata).await;
    shut_down(&host, &[&second_id]).await;

    assert!(
        written_with_the_approval,
        "the state that holds the approval was written before the create"
    );
    assert!(
        !answered_before_the_write,
        "the create does not answer while the write of the approvals it withdrew waits"
    );
    assert_eq!(created, second_id, "the mob is created under the id");
    assert!(
        !stored_after.contains(&of_an_earlier_mob),
        "the state in the store no longer holds the approval: {stored_after:?}"
    );
}

/// A release of a mob's schedules that cannot be done, in the place of the
/// one a launch's schedule service binds.
struct SchedulesThatCannotBeReleased;

#[async_trait::async_trait]
impl crate::mob_schedules_release::MobSchedulesRelease for SchedulesThatCannotBeReleased {
    async fn release_mob(&self, _mob_id: &MobId) -> Result<usize, String> {
        Err("the schedule store of the row is closed".to_string())
    }
}

/// A create that a release refuses leaves what the host's delivery service
/// keeps under the id as it was: that goes after every release that can
/// refuse the create. A route and a delivery are kept under an id no mob
/// has, and the launch's schedules cannot be released. The create of a mob
/// under that id is refused by the schedules' release, and the route and
/// the delivery are still kept under the id.
#[tokio::test]
async fn a_create_refused_by_a_release_leaves_the_routes_and_deliveries_kept_under_its_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _sessions) = launch(&temp, definition("host")).await;
    host.mob_runtime()
        .mob_schedules()
        .bind(Arc::new(SchedulesThatCannotBeReleased));
    let of_the_second = definition("second");
    let second_id = of_the_second.id.clone();
    {
        let module_runtime = host.module_runtime_handle();
        let mut module_runtime = module_runtime.lock().await;
        module_runtime
            .add_runtime_route(kept_route(Some(second_id.as_str())))
            .expect("the route is added");
        module_runtime.record_delivery_for_test(kept_delivery(
            "delivery-of-an-earlier-mob",
            Some(second_id.as_str()),
        ));
    }

    let created = Box::pin(set_of(&host).mob_create_definition(of_the_second)).await;
    let refused = created.as_ref().err().map(ToString::to_string);
    let still_kept = kept_for(&host, Some(second_id.as_str())).await;
    shut_down(&host, &[]).await;

    assert!(
        refused.as_deref().is_some_and(|answer| {
            answer.contains(crate::private_owners::CREATE_RELEASE_FAILED)
        }),
        "the create is refused by a release that failed (the schedules'): {refused:?}"
    );
    assert_eq!(
        still_kept,
        (true, vec!["delivery-of-an-earlier-mob".to_string()]),
        "the route and the delivery are still kept under the id"
    );
}
