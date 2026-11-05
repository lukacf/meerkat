//! Rows of the `decision/*` wire: listing, reads, renderings, presentation,
//! withdrawal, and the host adapter's decide.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use meerkat::AgentFactory;
use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionId, DecisionKind, DecisionOperationId, DecisionPhase,
    DecisionPrincipalKey, DecisionReference, DecisionRenderer, DecisionRendering,
    DecisionRequestBody, DecisionRow, DecisionStore, DecisionSubjectKey, PresentationAudience,
    PresentationRecord, Sha256Digest, StoredDecision,
};
use meerkat_core::{
    Config, ConfigStore, MemoryConfigStore, PrincipalKind, PrincipalRef, TrustDomainId,
};
use meerkat_runtime::decision::{DecisionError, DecisionRefusal};
use meerkat_store::SqliteDecisionStore;
use serde_json::json;

use super::decision_error_to_response;
use crate::protocol::{RpcId, RpcRequest, RpcResponse};
use crate::router::{MethodRouter, NotificationSink};
use crate::session_runtime::SessionRuntime;

const RENDERING: &str = "Release build 7 to production, with the change log attached.";
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;
const CAPABILITY_UNAVAILABLE: i32 = -32020;
const STORE_UNAVAILABLE: i32 = -32033;
const STORE_CONFLICT: i32 = -32034;
const COMMIT_UNKNOWN: i32 = -32035;

fn principal(kind: PrincipalKind, id: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        kind,
        id,
        TrustDomainId::new("example.org").expect("trust domain"),
    )
    .expect("principal")
}

fn requester_key() -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(&principal(PrincipalKind::PersonalAgent, "builder"))
        .expect("principal key")
}

fn subject_digest() -> String {
    Sha256Digest::of(RENDERING.as_bytes()).to_string()
}

/// The hold of `subject` of `kind` for `approver`, until far in the future.
fn hold_request(kind: &str, subject: &str, approver: &str) -> DecisionHoldRequest {
    hold_request_until(kind, subject, approver, u64::MAX / 2)
}

/// The hold of `subject` of `kind` for `approver`, until `deadline_at_ms`.
fn hold_request_until(
    kind: &str,
    subject: &str,
    approver: &str,
    deadline_at_ms: u64,
) -> DecisionHoldRequest {
    DecisionHoldRequest::new(
        DecisionKind::new(kind).expect("kind"),
        DecisionSubjectKey::new(subject).expect("subject key"),
        DecisionOperationId::new(format!("operation:{subject}")).expect("operation id"),
        Sha256Digest::of(br#"{"build":7}"#),
        DecisionRequestBody::from_value(&json!({"build": 7})).expect("request body"),
        requester_key(),
        ApproverId::new(approver).expect("approver id"),
        deadline_at_ms,
        DecisionRendering::new(RENDERING).expect("rendering"),
        DecisionRenderer::new("render.v1").expect("renderer"),
    )
}

/// The host wall clock in milliseconds, the clock a realm's decision owner
/// reads.
fn now_ms() -> u64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch");
    u64::try_from(elapsed.as_millis()).expect("milliseconds")
}

/// Sleep until the wall clock is past `at_ms`.
async fn wait_past(at_ms: u64) {
    while now_ms() <= at_ms {
        tokio::time::sleep(Duration::from_millis(at_ms.saturating_sub(now_ms()) + 10)).await;
    }
}

/// The persistence of a realm under `root` whose decisions are durable.
async fn realm_bundle(root: &Path) -> meerkat::PersistenceBundle {
    let (_, bundle) = meerkat::open_realm_persistence_in(
        root,
        "decisions",
        Some(meerkat_store::RealmBackend::Sqlite),
        None,
    )
    .await
    .expect("open the realm");
    bundle
}

/// The row of `decision_id` as the realm's decision store holds it, read
/// past the owner.
fn stored_row(store_path: &Path, decision_id: &DecisionId) -> StoredDecision {
    match SqliteDecisionStore::in_store_path(store_path)
        .get(decision_id)
        .expect("read the decision store")
        .expect("a stored row")
    {
        DecisionRow::Readable(row) => *row,
        DecisionRow::Unreadable(row) => panic!("the stored row is unreadable: {row:?}"),
    }
}

/// A router over a realm whose persistence keeps durable decisions.
async fn persistent_router(root: &Path) -> (MethodRouter, Arc<SessionRuntime>) {
    router_over(realm_bundle(root).await)
}

/// A router over `bundle`.
fn router_over(bundle: meerkat::PersistenceBundle) -> (MethodRouter, Arc<SessionRuntime>) {
    let runtime = Arc::new(
        SessionRuntime::new(
            AgentFactory::minimal(),
            Config::default(),
            4,
            bundle,
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime"),
    );
    let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
        Config::default(),
        meerkat_models::canonical(),
    ));
    let router = MethodRouter::new(Arc::clone(&runtime), config_store, NotificationSink::noop())
        .expect("construct the router");
    (router, runtime)
}

/// A router whose persistence keeps no durable decisions: a bundle composed
/// from stores alone.
async fn ephemeral_router() -> MethodRouter {
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let bundle = meerkat::PersistenceBundle::new(store, runtime_store, blob_store)
        .expect("construct runtime authority");
    let runtime = Arc::new(
        SessionRuntime::new(
            AgentFactory::minimal(),
            Config::default(),
            4,
            bundle,
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime"),
    );
    let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
        Config::default(),
        meerkat_models::canonical(),
    ));
    MethodRouter::new(runtime, config_store, NotificationSink::noop())
        .expect("construct the router")
}

fn request(method: &str, params: &serde_json::Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(RpcId::Num(1)),
        method: method.to_string(),
        params: Some(
            serde_json::value::RawValue::from_string(params.to_string()).expect("raw params"),
        ),
    }
}

async fn call(router: &MethodRouter, method: &str, params: serde_json::Value) -> RpcResponse {
    router
        .dispatch(request(method, &params))
        .await
        .expect("a response")
}

fn result(response: &RpcResponse) -> serde_json::Value {
    assert!(
        response.error.is_none(),
        "expected success, got {:?}",
        response.error
    );
    serde_json::from_str(response.result.as_ref().expect("a result").get()).expect("result json")
}

fn error_code(response: &RpcResponse) -> i32 {
    response.error.as_ref().expect("an error response").code
}

fn error_data(response: &RpcResponse) -> serde_json::Value {
    response
        .error
        .as_ref()
        .expect("an error response")
        .data
        .clone()
        .unwrap_or(serde_json::Value::Null)
}

/// A presentation of every part of the held rendering (one part) to the
/// approver id `team:owners`, at `at_ms`.
fn complete_presentation(at_ms: u64) -> serde_json::Value {
    json!({
        "audience": {"kind": "approver", "approver_id": "team:owners"},
        "subject_digest": subject_digest(),
        "parts_shown": [{"index": 0, "count": 1, "part_digest": subject_digest()}],
        "channel_ref": "chat:owners",
        "message_ref": "message-1",
        "at_ms": at_ms,
    })
}

fn decision_ids(listing: &serde_json::Value) -> Vec<String> {
    listing["decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .map(|decision| decision["decision_id"].as_str().expect("id").to_owned())
        .collect()
}

#[tokio::test]
async fn decision_list_pages_by_after_and_filters_by_kind_phase_and_approver() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let decisions = runtime.decision_service();
    let first = decisions
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");
    let second = decisions
        .hold(hold_request("release", "build-2", "team:owners"), false)
        .expect("hold");
    let third = decisions
        .hold(hold_request("deploy", "build-3", "team:ops"), false)
        .expect("hold");
    decisions
        .withdraw(
            &second.decision_id,
            &meerkat_runtime::decision::DecisionWithdrawer::Host,
        )
        .expect("withdraw");
    let mut all = vec![
        first.decision_id.to_string(),
        second.decision_id.to_string(),
        third.decision_id.to_string(),
    ];
    all.sort();

    let every = result(&call(&router, "decision/list", json!({})).await);
    assert_eq!(decision_ids(&every), all, "one page, in decision id order");
    assert_eq!(every["unreadable"], json!([]));
    assert!(every.get("next_after").is_none(), "{every}");

    let page = result(&call(&router, "decision/list", json!({"limit": 2})).await);
    assert_eq!(decision_ids(&page), all[..2].to_vec());
    let after = page["next_after"].as_str().expect("a next page").to_owned();
    assert_eq!(after, all[1]);
    let rest = result(
        &call(
            &router,
            "decision/list",
            json!({"limit": 2, "after": after}),
        )
        .await,
    );
    assert_eq!(decision_ids(&rest), all[2..].to_vec());
    assert!(rest.get("next_after").is_none(), "{rest}");

    let deploys = result(&call(&router, "decision/list", json!({"kind": "deploy"})).await);
    assert_eq!(decision_ids(&deploys), vec![third.decision_id.to_string()]);
    let withdrawn = result(&call(&router, "decision/list", json!({"phase": "withdrawn"})).await);
    assert_eq!(
        decision_ids(&withdrawn),
        vec![second.decision_id.to_string()]
    );
    assert_eq!(withdrawn["decisions"][0]["phase"], "withdrawn");
    let ops = result(&call(&router, "decision/list", json!({"approver_id": "team:ops"})).await);
    assert_eq!(decision_ids(&ops), vec![third.decision_id.to_string()]);
    let requested = result(
        &call(
            &router,
            "decision/list",
            json!({"requester": requester_key().to_string(), "kind": "release"}),
        )
        .await,
    );
    assert_eq!(decision_ids(&requested).len(), 2);

    // A filter value this version cannot read is refused, never read as no
    // filter.
    for params in [
        json!({"phase": "archived"}),
        json!({"kind": "Not A Kind"}),
        json!({"approver_id": "team owners"}),
        json!({"requester": "builder"}),
        json!({"limit": 2, "cursor": "x"}),
    ] {
        let refused = call(&router, "decision/list", params.clone()).await;
        assert_eq!(error_code(&refused), INVALID_PARAMS, "{params}");
    }
}

#[tokio::test]
async fn a_decision_page_of_no_decisions_is_invalid_params() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let decisions = runtime.decision_service();
    for subject in ["build-1", "build-2"] {
        decisions
            .hold(hold_request("release", subject, "team:owners"), false)
            .expect("hold");
    }

    let refused = call(&router, "decision/list", json!({"limit": 0})).await;
    assert_eq!(error_code(&refused), INVALID_PARAMS, "{refused:?}");
    assert!(error_data(&refused).is_null(), "{refused:?}");

    let one = result(&call(&router, "decision/list", json!({"limit": 1})).await);
    assert_eq!(decision_ids(&one).len(), 1);
    assert!(one.get("next_after").is_some(), "{one}");
    // A larger page than the store's reads as the store's.
    let all = result(&call(&router, "decision/list", json!({"limit": u32::MAX})).await);
    assert_eq!(decision_ids(&all).len(), 2);
}

#[tokio::test]
async fn plain_dispatch_has_no_route_to_decision_decide() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let decisions = runtime.decision_service();
    let held = decisions
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");
    let presentation = PresentationRecord::complete(
        PresentationAudience::Approver {
            approver_id: ApproverId::new("team:owners").expect("approver id"),
        },
        &held.descriptor.rendering,
        None,
        DecisionReference::new("chat:owners").expect("channel"),
        DecisionReference::new("message-1").expect("message"),
        2_000,
    );
    decisions
        .present(&held.decision_id, &presentation)
        .expect("present");

    let decided = call(
        &router,
        "decision/decide",
        json!({
            "decision_id": held.decision_id.to_string(),
            "approve": true,
            "presented_digest": subject_digest(),
        }),
    )
    .await;
    assert_eq!(error_code(&decided), METHOD_NOT_FOUND, "{decided:?}");
    let row = decisions.get(&held.decision_id).expect("read");
    assert_eq!(row.state.phase, DecisionPhase::Open);
    assert!(row.outcome.is_none());
}

#[tokio::test]
async fn decision_get_reads_one_decision_and_answers_decision_not_found() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let held = runtime
        .decision_service()
        .hold(hold_request("release", "build-1", "team:owners"), true)
        .expect("hold");

    let read = result(
        &call(
            &router,
            "decision/get",
            json!({"decision_id": held.decision_id.to_string()}),
        )
        .await,
    );
    let decision = &read["decision"];
    assert_eq!(decision["decision_id"], held.decision_id.to_string());
    assert_eq!(decision["kind"], "release");
    assert_eq!(decision["subject_key"], "build-1");
    assert_eq!(decision["phase"], "open");
    assert_eq!(decision["stage"], "human");
    assert_eq!(decision["notice"], "not_due");
    assert_eq!(decision["approver_id"], "team:owners");
    assert_eq!(decision["requester"], requester_key().to_string());
    assert_eq!(decision["subject_digest"], subject_digest());
    assert_eq!(decision["renderer"], "render.v1");
    assert_eq!(decision["revision"], 1);
    assert!(
        decision.get("request").is_none(),
        "the request only when asked: {decision}"
    );

    let with_request = result(
        &call(
            &router,
            "decision/get",
            json!({"decision_id": held.decision_id.to_string(), "include_request": true}),
        )
        .await,
    );
    assert_eq!(with_request["decision"]["request"], json!({"build": 7}));

    let unknown = format!("decision:{}", "0".repeat(64));
    let missing = call(&router, "decision/get", json!({"decision_id": unknown})).await;
    assert_eq!(error_code(&missing), INVALID_PARAMS);
    assert_eq!(error_data(&missing), json!({"kind": "decision_not_found"}));

    let malformed = call(&router, "decision/get", json!({"decision_id": "build-1"})).await;
    assert_eq!(error_code(&malformed), INVALID_PARAMS);
}

#[tokio::test]
async fn decision_rendering_answers_the_canonical_parts_of_a_part_size() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let held = runtime
        .decision_service()
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");
    let decision_id = held.decision_id.to_string();

    let whole = result(
        &call(
            &router,
            "decision/rendering",
            json!({"decision_id": decision_id}),
        )
        .await,
    );
    assert_eq!(whole["rendering"], RENDERING);
    assert_eq!(whole["renderer"], "render.v1");
    assert_eq!(whole["subject_digest"], subject_digest());
    assert_eq!(
        whole["parts"],
        json!([{"index": 0, "count": 1, "part_digest": subject_digest()}])
    );
    assert!(whole.get("max_part_bytes").is_none(), "{whole}");

    let split = result(
        &call(
            &router,
            "decision/rendering",
            json!({"decision_id": decision_id, "max_part_bytes": 16}),
        )
        .await,
    );
    assert_eq!(split["max_part_bytes"], 16);
    let parts = split["parts"].as_array().expect("parts");
    let expected: Vec<serde_json::Value> = RENDERING
        .as_bytes()
        .chunks(16)
        .enumerate()
        .map(|(index, bytes)| {
            json!({
                "index": index,
                "count": RENDERING.len().div_ceil(16),
                "part_digest": Sha256Digest::of(bytes).to_string(),
            })
        })
        .collect();
    assert_eq!(parts, &expected);

    let zero = call(
        &router,
        "decision/rendering",
        json!({"decision_id": decision_id, "max_part_bytes": 0}),
    )
    .await;
    assert_eq!(error_code(&zero), INVALID_PARAMS);
}

#[tokio::test]
async fn a_decision_refusal_answers_decision_refused_with_its_kind() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let held = runtime
        .decision_service()
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");
    let other = Sha256Digest::of(b"another rendering").to_string();
    let mut presentation = complete_presentation(2_000);
    presentation["subject_digest"] = json!(other);
    presentation["parts_shown"][0]["part_digest"] = json!(other);

    let refused = call(
        &router,
        "decision/present",
        json!({"decision_id": held.decision_id.to_string(), "presentation": presentation}),
    )
    .await;
    assert_eq!(error_code(&refused), INVALID_PARAMS, "{refused:?}");
    assert_eq!(
        error_data(&refused),
        json!({
            "kind": "decision_refused",
            "decision_refusal": {"kind": "subject_changed"},
        })
    );
}

#[tokio::test]
async fn a_presentation_at_zero_is_refused_presentation_incomplete_over_the_wire() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let held = runtime
        .decision_service()
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");

    let refused = call(
        &router,
        "decision/present",
        json!({"decision_id": held.decision_id.to_string(), "presentation": complete_presentation(0)}),
    )
    .await;
    assert_eq!(error_code(&refused), INVALID_PARAMS, "{refused:?}");
    assert_eq!(
        error_data(&refused),
        json!({
            "kind": "decision_refused",
            "decision_refusal": {"kind": "presentation_incomplete"},
        })
    );
    let row = runtime
        .decision_service()
        .get(&held.decision_id)
        .expect("read");
    assert!(row.presentations.is_empty());
    assert_eq!(row.state.first_presented_to_approver_at_ms, None);
}

#[tokio::test]
async fn a_host_presentation_is_recorded_over_the_wire() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let held = runtime
        .decision_service()
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");

    let presented = result(
        &call(
            &router,
            "decision/present",
            json!({"decision_id": held.decision_id.to_string(), "presentation": complete_presentation(2_000)}),
        )
        .await,
    );
    let decision = &presented["decision"];
    assert_eq!(decision["phase"], "open");
    assert_eq!(decision["first_presented_to_approver_at_ms"], 2_000);
    assert_eq!(
        decision["presentations"],
        json!([{
            "audience": {"kind": "approver", "approver_id": "team:owners"},
            "subject_digest": subject_digest(),
            "parts_shown": [{"index": 0, "count": 1, "part_digest": subject_digest()}],
            "channel_ref": "chat:owners",
            "message_ref": "message-1",
            "at_ms": 2_000,
        }])
    );

    // A request the wire cannot read as a presentation is refused before
    // the owner is asked.
    let mut unknown_audience = complete_presentation(2_000);
    unknown_audience["audience"] = json!({"kind": "everyone"});
    let refused = call(
        &router,
        "decision/present",
        json!({"decision_id": held.decision_id.to_string(), "presentation": unknown_audience}),
    )
    .await;
    assert_eq!(error_code(&refused), INVALID_PARAMS);
}

#[tokio::test]
async fn a_withdraw_on_the_wire_is_the_hosts() {
    let root = tempfile::tempdir().expect("tempdir");
    let (router, runtime) = persistent_router(root.path()).await;
    let held = runtime
        .decision_service()
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");
    let decision_id = held.decision_id.to_string();

    let withdrawn = result(
        &call(
            &router,
            "decision/withdraw",
            json!({"decision_id": decision_id}),
        )
        .await,
    );
    assert_eq!(withdrawn["decision"]["phase"], "withdrawn");

    let again = call(
        &router,
        "decision/withdraw",
        json!({"decision_id": decision_id}),
    )
    .await;
    assert_eq!(error_code(&again), INVALID_PARAMS);
    assert_eq!(
        error_data(&again),
        json!({"kind": "decision_refused", "decision_refusal": {"kind": "not_open"}})
    );
}

#[tokio::test]
async fn an_ephemeral_runtime_answers_decision_methods_capability_unavailable() {
    let router = ephemeral_router().await;
    let decision_id = format!("decision:{}", "0".repeat(64));
    for (method, params) in [
        ("decision/list", json!({})),
        ("decision/get", json!({"decision_id": decision_id})),
        ("decision/rendering", json!({"decision_id": decision_id})),
        (
            "decision/present",
            json!({"decision_id": decision_id, "presentation": complete_presentation(2_000)}),
        ),
        ("decision/withdraw", json!({"decision_id": decision_id})),
    ] {
        let answer = call(&router, method, params).await;
        assert_eq!(
            error_code(&answer),
            CAPABILITY_UNAVAILABLE,
            "{method}: {answer:?}"
        );
        assert_eq!(
            error_data(&answer),
            json!({"kind": "no_decision_store"}),
            "{method}"
        );
    }
}

#[tokio::test]
async fn a_candidate_launch_reads_decisions_and_refuses_every_write() {
    let root = tempfile::tempdir().expect("tempdir");
    let bundle = realm_bundle(root.path()).await;
    let store_path = bundle
        .store_path()
        .expect("a realm's store path")
        .to_path_buf();
    // Held by the admitted owner, before the launch narrows it.
    let admitted = bundle.decision_owner();
    let held = admitted
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");
    let deadline_at_ms = now_ms() + 1_000;
    let lapsing = admitted
        .hold(
            hold_request_until("release", "build-2", "team:owners", deadline_at_ms),
            false,
        )
        .expect("hold");
    let (router, _runtime) = router_over(bundle.with_candidate_launch_decisions());
    let decision_id = held.decision_id.to_string();

    // Reads answer.
    let listing = result(&call(&router, "decision/list", json!({})).await);
    let mut all = vec![decision_id.clone(), lapsing.decision_id.to_string()];
    all.sort();
    assert_eq!(decision_ids(&listing), all);
    let read = result(&call(&router, "decision/get", json!({"decision_id": decision_id})).await);
    assert_eq!(read["decision"]["phase"], "open");
    let rendering = result(
        &call(
            &router,
            "decision/rendering",
            json!({"decision_id": decision_id}),
        )
        .await,
    );
    assert_eq!(rendering["rendering"], RENDERING);

    // Every write is refused, typed, and nothing is written.
    for (method, params) in [
        (
            "decision/present",
            json!({"decision_id": decision_id, "presentation": complete_presentation(2_000)}),
        ),
        ("decision/withdraw", json!({"decision_id": decision_id})),
    ] {
        let refused = call(&router, method, params).await;
        assert_eq!(
            error_code(&refused),
            CAPABILITY_UNAVAILABLE,
            "{method}: {refused:?}"
        );
        assert_eq!(
            error_data(&refused),
            json!({"kind": "candidate_launch"}),
            "{method}"
        );
    }
    assert_eq!(stored_row(&store_path, &held.decision_id), held);

    // Past its deadline a decision reads as expired, and nothing records the
    // expiry.
    wait_past(deadline_at_ms).await;
    let lapsed = result(
        &call(
            &router,
            "decision/get",
            json!({"decision_id": lapsing.decision_id.to_string()}),
        )
        .await,
    );
    assert_eq!(lapsed["decision"]["phase"], "expired");
    assert_eq!(lapsed["decision"]["revision"], lapsing.revision);
    assert_eq!(stored_row(&store_path, &lapsing.decision_id), lapsing);
}

#[tokio::test]
async fn a_torn_decision_row_answers_decision_unreadable() {
    let root = tempfile::tempdir().expect("tempdir");
    let bundle = realm_bundle(root.path()).await;
    let store_path = bundle
        .store_path()
        .expect("a realm's store path")
        .to_path_buf();
    let (router, runtime) = router_over(bundle);
    let held = runtime
        .decision_service()
        .hold(hold_request("release", "build-1", "team:owners"), false)
        .expect("hold");
    let decision_id = held.decision_id.to_string();
    // An open decision with a permit outstanding breaks an invariant of its
    // machine: the row is torn.
    let mut torn = held.clone();
    torn.state.permit_issued = true;
    SqliteDecisionStore::in_store_path(&store_path)
        .replace_if_revision(held.revision, &torn)
        .expect("tear the row");

    for method in ["decision/get", "decision/rendering"] {
        let unreadable = call(&router, method, json!({"decision_id": decision_id})).await;
        assert_eq!(
            error_code(&unreadable),
            INTERNAL_ERROR,
            "{method}: {unreadable:?}"
        );
        assert_eq!(
            error_data(&unreadable),
            json!({"kind": "decision_unreadable"}),
            "{method}"
        );
    }
    let listing = result(&call(&router, "decision/list", json!({})).await);
    assert_eq!(listing["decisions"], json!([]));
    let unreadable = listing["unreadable"].as_array().expect("unreadable rows");
    assert_eq!(unreadable.len(), 1, "{listing}");
    assert_eq!(unreadable[0]["decision_id"], decision_id);
}

fn a_decision_id() -> DecisionId {
    DecisionId::derive(
        &DecisionKind::new("release").expect("kind"),
        &DecisionSubjectKey::new("build-1").expect("subject key"),
    )
}

#[test]
fn a_decision_error_answers_its_code_and_typed_data() {
    let decision_id = a_decision_id();
    let invalid = DecisionKind::new("Not A Kind").expect_err("an invalid kind");
    let none = serde_json::Value::Null;
    for (error_value, code, data) in [
        (
            DecisionError::NotFound(decision_id.clone()),
            INVALID_PARAMS,
            json!({"kind": "decision_not_found"}),
        ),
        (
            DecisionError::Unreadable {
                decision_id: decision_id.to_string(),
                reason: "torn".to_owned(),
            },
            INTERNAL_ERROR,
            json!({"kind": "decision_unreadable"}),
        ),
        (
            DecisionError::CandidateLaunch,
            CAPABILITY_UNAVAILABLE,
            json!({"kind": "candidate_launch"}),
        ),
        // Store faults keep their commitment truth apart, with the approval
        // store codes.
        (
            DecisionError::Unavailable("the store is locked".to_owned()),
            STORE_UNAVAILABLE,
            none.clone(),
        ),
        (
            DecisionError::Unsettled(decision_id.clone()),
            STORE_UNAVAILABLE,
            none.clone(),
        ),
        (
            DecisionError::Conflict(decision_id.clone()),
            STORE_CONFLICT,
            none.clone(),
        ),
        (
            DecisionError::CommitUnknown(decision_id),
            COMMIT_UNKNOWN,
            none.clone(),
        ),
        (
            DecisionError::Invalid(invalid),
            INVALID_PARAMS,
            none.clone(),
        ),
        (DecisionError::NotAReceipt, INVALID_PARAMS, none.clone()),
        (DecisionError::Clock, INTERNAL_ERROR, none.clone()),
        (
            DecisionError::Machine("no transition".to_owned()),
            INTERNAL_ERROR,
            none.clone(),
        ),
        (DecisionError::ExecutorHeld, INTERNAL_ERROR, none),
    ] {
        let answer = decision_error_to_response(Some(RpcId::Num(1)), error_value.clone());
        assert_eq!(error_code(&answer), code, "{error_value:?}");
        assert_eq!(error_data(&answer), data, "{error_value:?}");
    }
}

#[test]
fn every_decision_refusal_answers_decision_refused_with_its_kind() {
    let refusals = [
        (DecisionRefusal::InvalidWindow, "invalid_window"),
        (DecisionRefusal::SelfReview, "self_review"),
        (DecisionRefusal::HoldConflict, "hold_conflict"),
        (DecisionRefusal::NotOpen, "not_open"),
        (DecisionRefusal::Expired, "expired"),
        (DecisionRefusal::SubjectChanged, "subject_changed"),
        (
            DecisionRefusal::PresentationIncomplete,
            "presentation_incomplete",
        ),
        (DecisionRefusal::WrongAudience, "wrong_audience"),
        (DecisionRefusal::ReviewStage, "review_stage"),
        (DecisionRefusal::HumanStage, "human_stage"),
        (DecisionRefusal::EvidenceRequired, "evidence_required"),
        (DecisionRefusal::NotEligible, "not_eligible"),
        (DecisionRefusal::SelfApproval, "self_approval"),
        (DecisionRefusal::WrongReviewer, "wrong_reviewer"),
        (DecisionRefusal::NotRequester, "not_requester"),
        (DecisionRefusal::NotApproved, "not_approved"),
        (DecisionRefusal::BindingMismatch, "binding_mismatch"),
        (DecisionRefusal::NotAuthorized, "not_authorized"),
        (DecisionRefusal::SettlementOwed, "settlement_owed"),
        (DecisionRefusal::NotOwed, "not_owed"),
        (
            DecisionRefusal::DecidedBeforePresentation,
            "decided_before_presentation",
        ),
    ];
    let distinct: std::collections::HashSet<DecisionRefusal> =
        refusals.iter().map(|(refusal, _)| *refusal).collect();
    let kinds: std::collections::HashSet<&str> = refusals.iter().map(|(_, kind)| *kind).collect();
    assert_eq!(
        (distinct.len(), kinds.len()),
        (21, 21),
        "every refusal once"
    );
    for (refusal, kind) in refusals {
        let answer = decision_error_to_response(
            Some(RpcId::Num(1)),
            DecisionError::Refused {
                decision_id: a_decision_id(),
                refusal,
            },
        );
        assert_eq!(error_code(&answer), INVALID_PARAMS, "{kind}");
        assert_eq!(
            error_data(&answer),
            json!({"kind": "decision_refused", "decision_refusal": {"kind": kind}}),
            "{kind}"
        );
    }
}

// --- The host adapter's decide: these rows use the authenticated event API,
// --- so they compile only where it exists.
mod authenticated_event {
    use std::sync::Arc;

    use meerkat_core::ApprovalAssurance;
    use meerkat_core::PrincipalKind;
    use meerkat_core::approval::consent::{
        AuthenticatedApprover, ConsentEligibility, ConsentIneligible, HostAuthenticationScope,
    };
    use meerkat_core::approval::decision::{
        ApproverId, AttestedDecision, AuthenticatedDecisionEvent, DecisionId, DecisionIngress,
        DecisionPhase, DecisionPrincipalKey, DecisionReference, Sha256Digest,
    };
    use serde_json::json;

    use super::{
        INVALID_PARAMS, METHOD_NOT_FOUND, complete_presentation, error_code, error_data,
        hold_request, persistent_router, principal, request, result, subject_digest,
    };
    use crate::router::MethodRouter;

    /// Eligibility that is always current.
    struct Eligible;

    impl ConsentEligibility for Eligible {
        fn check_current(&self) -> Result<(), ConsentIneligible> {
            Ok(())
        }
    }

    /// Alice's decision on `decision_id`, authenticated on a chat event the
    /// chat observed at `observed_at_ms`, in the login `login-7`.
    fn event(
        decision_id: &DecisionId,
        approve: bool,
        observed_at_ms: u64,
    ) -> AuthenticatedDecisionEvent {
        let approver = AuthenticatedApprover::new(
            principal(PrincipalKind::Human, "alice"),
            ApprovalAssurance::PersonAuthenticated,
        )
        .expect("approver")
        .with_authentication_scope(HostAuthenticationScope::new("login-7"));
        let decision = AttestedDecision::new(
            approver,
            DecisionIngress::new("chat").expect("ingress"),
            DecisionReference::new("chat-event-1").expect("event ref"),
            observed_at_ms,
            ApproverId::new("team:owners").expect("approver id"),
            Sha256Digest::new(subject_digest()).expect("digest"),
            approve,
        )
        .expect("attested decision");
        AuthenticatedDecisionEvent::new(decision_id.clone(), decision, Arc::new(Eligible))
    }

    fn decide_body(decision_id: &DecisionId, approve: bool) -> serde_json::Value {
        json!({
            "decision_id": decision_id.to_string(),
            "approve": approve,
            "presented_digest": subject_digest(),
        })
    }

    async fn decide(
        router: &MethodRouter,
        body: &serde_json::Value,
        event: AuthenticatedDecisionEvent,
    ) -> crate::protocol::RpcResponse {
        router
            .dispatch_with_decision_event(request("decision/decide", body), event)
            .await
    }

    async fn present(router: &MethodRouter, decision_id: &DecisionId, at_ms: u64) {
        let presented = super::call(
            router,
            "decision/present",
            json!({"decision_id": decision_id.to_string(), "presentation": complete_presentation(at_ms)}),
        )
        .await;
        assert!(presented.error.is_none(), "{presented:?}");
    }

    #[tokio::test]
    async fn a_decide_body_that_names_an_approver_is_invalid_params() {
        let root = tempfile::tempdir().expect("tempdir");
        let (router, runtime) = persistent_router(root.path()).await;
        let held = runtime
            .decision_service()
            .hold(hold_request("release", "build-1", "team:owners"), false)
            .expect("hold");
        present(&router, &held.decision_id, 2_000).await;

        for field in [
            "approver",
            "approver_id",
            "actor",
            "assurance",
            "authentication_scope",
        ] {
            let mut body = decide_body(&held.decision_id, true);
            body[field] = json!("someone");
            let refused = decide(&router, &body, event(&held.decision_id, true, 3_000)).await;
            assert_eq!(error_code(&refused), INVALID_PARAMS, "{field}: {refused:?}");
        }
        let row = runtime
            .decision_service()
            .get(&held.decision_id)
            .expect("read");
        assert_eq!(row.state.phase, DecisionPhase::Open, "nothing was decided");
    }

    #[tokio::test]
    async fn an_adapter_event_whose_decision_differs_from_the_body_is_invalid_params() {
        let root = tempfile::tempdir().expect("tempdir");
        let (router, runtime) = persistent_router(root.path()).await;
        let decisions = runtime.decision_service();
        let held = decisions
            .hold(hold_request("release", "build-1", "team:owners"), false)
            .expect("hold");
        let other = decisions
            .hold(hold_request("release", "build-2", "team:owners"), false)
            .expect("hold");
        present(&router, &held.decision_id, 2_000).await;

        let mut other_decision = decide_body(&held.decision_id, true);
        other_decision["decision_id"] = json!(other.decision_id.to_string());
        let mut other_digest = decide_body(&held.decision_id, true);
        other_digest["presented_digest"] =
            json!(Sha256Digest::of(b"another rendering").to_string());
        let mut with_reason = decide_body(&held.decision_id, true);
        with_reason["reason"] = json!("looks fine");
        for body in [
            other_decision,
            decide_body(&held.decision_id, false),
            other_digest,
            with_reason,
        ] {
            let refused = decide(&router, &body, event(&held.decision_id, true, 3_000)).await;
            assert_eq!(error_code(&refused), INVALID_PARAMS, "{body}: {refused:?}");
            assert!(
                error_data(&refused).is_null(),
                "refused before the owner is asked"
            );
        }
        for decision_id in [&held.decision_id, &other.decision_id] {
            let row = decisions.get(decision_id).expect("read");
            assert_eq!(row.state.phase, DecisionPhase::Open, "nothing was decided");
        }
    }

    #[tokio::test]
    async fn a_decide_observed_before_the_first_presentation_is_refused_over_the_wire() {
        let root = tempfile::tempdir().expect("tempdir");
        let (router, runtime) = persistent_router(root.path()).await;
        let held = runtime
            .decision_service()
            .hold(hold_request("release", "build-1", "team:owners"), false)
            .expect("hold");
        present(&router, &held.decision_id, 2_000).await;

        let early = decide(
            &router,
            &decide_body(&held.decision_id, true),
            event(&held.decision_id, true, 1_000),
        )
        .await;
        assert_eq!(error_code(&early), INVALID_PARAMS, "{early:?}");
        assert_eq!(
            error_data(&early),
            json!({
                "kind": "decision_refused",
                "decision_refusal": {"kind": "decided_before_presentation"},
            })
        );

        let decided = result(
            &decide(
                &router,
                &decide_body(&held.decision_id, true),
                event(&held.decision_id, true, 3_000),
            )
            .await,
        );
        let decision = &decided["decision"];
        assert_eq!(decision["phase"], "approved");
        let outcome = &decision["outcome"];
        assert_eq!(outcome["tier"], "human");
        let evidence = &outcome["evidence"];
        assert_eq!(evidence["kind"], "human");
        let alice = DecisionPrincipalKey::of(&principal(PrincipalKind::Human, "alice"))
            .expect("principal key");
        assert_eq!(evidence["decider"], alice.to_string());
        assert_eq!(evidence["assurance"], "person_authenticated");
        assert_eq!(evidence["ingress"], "chat");
        assert_eq!(evidence["event_ref"], "chat-event-1");
        assert_eq!(evidence["observed_at_ms"], 3_000);
        assert_eq!(evidence["approve"], true);
        // The login the event was authenticated in is never projected.
        assert!(
            !decided.to_string().contains("login-7"),
            "no authentication scope on the wire: {decided}"
        );

        // The first valid decision wins.
        let second = decide(
            &router,
            &decide_body(&held.decision_id, false),
            event(&held.decision_id, false, 4_000),
        )
        .await;
        assert_eq!(
            error_data(&second),
            json!({"kind": "decision_refused", "decision_refusal": {"kind": "not_open"}})
        );
    }

    #[tokio::test]
    async fn an_authenticated_event_only_decides_decision_decide() {
        let root = tempfile::tempdir().expect("tempdir");
        let (router, runtime) = persistent_router(root.path()).await;
        let held = runtime
            .decision_service()
            .hold(hold_request("release", "build-1", "team:owners"), false)
            .expect("hold");
        present(&router, &held.decision_id, 2_000).await;

        let answer = router
            .dispatch_with_decision_event(
                request(
                    "decision/get",
                    &json!({"decision_id": held.decision_id.to_string()}),
                ),
                event(&held.decision_id, true, 3_000),
            )
            .await;
        assert_eq!(error_code(&answer), METHOD_NOT_FOUND, "{answer:?}");
        let row = runtime
            .decision_service()
            .get(&held.decision_id)
            .expect("read");
        assert_eq!(row.state.phase, DecisionPhase::Open);
    }
}
