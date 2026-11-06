//! The Flow Editor RPC on the Console's listener checks its caller as a
//! console change does. A change (a draft store write, a deploy with
//! `execute`, a validation through `rkat`) made for another caller than its
//! token is refused with `caller_changed`; one whose caller's login ended
//! while it waited is refused with `caller_not_current` right before it
//! writes the draft store or runs `rkat`, and nothing is written or run.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use super::console_human_admission_tests::{BOB, state};
use super::console_sign_out_tests::console_answer;
use super::console_token_lapse_tests::{durable_fences, human_claims, now_seconds, token};
use super::*;

/// The Flow Editor a runtime mounts on the Console's listener, under the
/// enforced console's decisions and `fences`, with the host's opt-in to
/// host deploys, so a caller names its own draft store file and may deploy.
fn flow_editor(fences: crate::auth::session_fence::ConsoleSessionFences) -> Router {
    crate::http_flow_editor::protected_flow_editor_router_with_runtime_catalog(
        state().decisions,
        crate::mobpack::MobpackRuntimeCatalogState::default(),
        None,
        true,
        fences,
        None,
        false,
    )
}

/// `method` with `params` on `/flow-editor/rpc` with `token` in the
/// `Authorization` header: the HTTP status and the JSON-RPC answer.
async fn flow_editor_rpc(
    app: &Router,
    token: &str,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/flow-editor/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": "flow-editor",
                        "method": method,
                        "params": params,
                    })
                    .to_string(),
                ))
                .expect("flow editor request"),
        )
        .await
        .expect("flow editor response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("flow editor body");
    (
        status,
        serde_json::from_slice(&body).expect("flow editor answer"),
    )
}

fn bob_token() -> String {
    token(human_claims(), Some(now_seconds() + 3600))
}

fn robot_token() -> String {
    token(
        json!({ "sub": "svc:robot", "email": "svc:robot", "actor_type": "service" }),
        Some(now_seconds() + 3600),
    )
}

/// The `caller_scope` `meta/console/whoami` reports for `token`.
async fn caller_scope(token: &str) -> Value {
    let (_, answer) = console_answer(
        &console_json_router_with_state(state()),
        Some(token),
        "meta/console/whoami",
        json!({}),
    )
    .await;
    answer["result"]["caller_scope"].clone()
}

/// The drafts the store at `store_path` holds.
async fn drafts(app: &Router, token: &str, store_path: &std::path::Path) -> usize {
    let (_, listed) = flow_editor_rpc(
        app,
        token,
        "meta/mobpacks/list",
        json!({ "store_path": store_path }),
    )
    .await;
    listed["result"]["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("a draft listing: {listed}"))
        .len()
}

/// The draft store lock, held on a thread of its own until released, so a
/// draft store method waits for it.
struct DraftStoreHold {
    release: std::sync::mpsc::Sender<()>,
    holder: std::thread::JoinHandle<()>,
}

impl DraftStoreHold {
    fn hold() -> Self {
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _held = crate::mobpack::hold_draft_store_lock_for_test();
            held_tx.send(()).expect("held");
            release_rx.recv().ok();
        });
        held_rx.recv().expect("the draft store lock is held");
        Self { release, holder }
    }

    fn release(self) {
        self.release.send(()).ok();
        self.holder.join().expect("the lock holder ends");
    }
}

/// Let a request spawned on this test's runtime pass its gate: it resolves
/// and checks its caller before its first await, and then waits for the
/// draft store on the blocking pool.
async fn let_the_request_reach_the_draft_store() {
    tokio::time::sleep(Duration::from_millis(250)).await;
}

/// A draft write made for another caller than its token is refused with
/// `caller_changed` and writes nothing; the caller's own scope passes, and
/// a read ignores the field.
#[tokio::test]
async fn flow_editor_change_with_other_expected_scope_is_refused() {
    let (_dir, fences) = durable_fences();
    let app = flow_editor(fences);
    let store = tempfile::tempdir().expect("draft store directory");
    let store_path = store.path().join("drafts.json");
    let bob = bob_token();
    let robot_scope = caller_scope(&robot_token()).await;
    let create = |expected: &Value| {
        json!({
            "template": "blank",
            "name": "Caller check",
            "store_path": store_path,
            "expected_caller_scope": expected,
        })
    };

    let (status, refused) =
        flow_editor_rpc(&app, &bob, "meta/mobpacks/create", create(&robot_scope)).await;
    assert_eq!(status, StatusCode::OK, "{refused}");
    assert_eq!(refused["error"]["code"], json!(-32056), "{refused}");
    assert_eq!(
        refused["error"]["data"]["kind"], "caller_changed",
        "{refused}"
    );
    assert_eq!(
        drafts(&app, &bob, &store_path).await,
        0,
        "nothing was written"
    );

    let (status, created) = flow_editor_rpc(
        &app,
        &bob,
        "meta/mobpacks/create",
        create(&caller_scope(&bob).await),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert!(created["error"].is_null(), "{created}");
    assert_eq!(drafts(&app, &bob, &store_path).await, 1);

    let (status, listed) = flow_editor_rpc(
        &app,
        &bob,
        "meta/mobpacks/list",
        json!({ "store_path": store_path, "expected_caller_scope": robot_scope }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(
        listed["error"].is_null(),
        "a read ignores the field: {listed}"
    );
}

/// A draft write whose caller's sessions are revoked while it waits for the
/// draft store is refused with `caller_not_current` right before it writes,
/// and nothing is written.
#[tokio::test]
async fn flow_editor_change_rechecks_caller_after_await() {
    let (_dir, fences) = durable_fences();
    let app = flow_editor(fences.clone());
    let store = tempfile::tempdir().expect("draft store directory");
    let store_path = store.path().join("drafts.json");
    let bob = bob_token();

    let hold = DraftStoreHold::hold();
    let pending = tokio::spawn({
        let app = app.clone();
        let bob = bob.clone();
        let params = json!({
            "template": "blank",
            "name": "Written after the login ended",
            "store_path": store_path,
        });
        async move { flow_editor_rpc(&app, &bob, "meta/mobpacks/create", params).await }
    });
    let_the_request_reach_the_draft_store().await;
    fences
        .revoke_sessions(BOB, std::time::SystemTime::now())
        .expect("revoke");
    hold.release();
    let (status, answer) = tokio::time::timeout(Duration::from_secs(60), pending)
        .await
        .expect("the write answers")
        .expect("the request task");
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["error"]["code"], json!(-32056), "{answer}");
    assert_eq!(
        answer["error"]["data"]["kind"], "caller_not_current",
        "{answer}"
    );
    assert!(!store_path.exists(), "nothing was written");
}

/// An `rkat` that appends one line to `marker` for every run and answers
/// like a completed `rkat mob run`.
#[cfg(unix)]
fn recording_rkat(dir: &std::path::Path, marker: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let rkat = dir.join("rkat");
    std::fs::write(
        &rkat,
        format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> '{}'\n\
             printf 'run\\tmob=docs\\tflow=main\\trun_id=run-1\\tstatus=completed\\n'\n\
             printf 'result\\t{{\"reply\":\"OK\"}}\\n'\n",
            marker.to_string_lossy()
        ),
    )
    .expect("write recording rkat");
    let mut permissions = std::fs::metadata(&rkat)
        .expect("recording rkat metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&rkat, permissions).expect("chmod recording rkat");
    rkat
}

/// A deploy with `execute` whose caller's sessions are revoked while it
/// reads its draft guard is refused with `caller_not_current` right before
/// it writes the archive and runs `rkat`: nothing runs.
#[cfg(unix)]
#[tokio::test]
async fn flow_editor_deploy_rechecks_caller_before_it_runs() {
    let (_dir, fences) = durable_fences();
    let app = flow_editor(fences.clone());
    let work = tempfile::tempdir().expect("work directory");
    let store_path = work.path().join("drafts.json");
    let deploy_dir = work.path().join("deploy");
    let marker = work.path().join("rkat.runs");
    let rkat = recording_rkat(work.path(), &marker);
    let bob = bob_token();

    let (_, catalogs) = flow_editor_rpc(&app, &bob, "meta/mobpacks/catalogs", json!({})).await;
    let document = catalogs["result"]["sample_mobpacks"]
        .as_array()
        .unwrap_or_else(|| panic!("sample mobpacks: {catalogs}"))
        .iter()
        .find(|sample| sample["id"] == "sample_docs_only")
        .expect("the docs sample")["document"]
        .clone();
    let (_, saved) = flow_editor_rpc(
        &app,
        &bob,
        "meta/mobpacks/save",
        json!({ "id": "deploy-check", "document": document, "store_path": store_path }),
    )
    .await;
    assert!(saved["error"].is_null(), "{saved}");
    let revision = saved["result"]["row"]["revision"].clone();

    let hold = DraftStoreHold::hold();
    let pending = tokio::spawn({
        let app = app.clone();
        let bob = bob.clone();
        let params = json!({
            "id": "deploy-check",
            "document": document,
            "expected_revision": revision,
            "store_path": store_path,
            "prompt": "Reply with exactly OK.",
            "execute": true,
            "output_dir": deploy_dir,
            "rkat_bin": rkat,
        });
        async move { flow_editor_rpc(&app, &bob, "meta/mobpacks/deploy", params).await }
    });
    let_the_request_reach_the_draft_store().await;
    fences
        .revoke_sessions(BOB, std::time::SystemTime::now())
        .expect("revoke");
    hold.release();
    let (status, answer) = tokio::time::timeout(Duration::from_secs(60), pending)
        .await
        .expect("the deploy answers")
        .expect("the request task");
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["error"]["code"], json!(-32056), "{answer}");
    assert_eq!(
        answer["error"]["data"]["kind"], "caller_not_current",
        "{answer}"
    );
    assert!(!marker.exists(), "rkat never ran");
    assert!(!deploy_dir.exists(), "no archive was written");
}

/// The Flow Editor's capabilities on the Console's listener name the caller
/// the request resolved to, the scope `meta/console/whoami` reports to the
/// same caller; the standalone server has no caller and names none. A field
/// that is not a string is invalid params.
#[tokio::test]
async fn flow_editor_capabilities_name_the_caller() {
    let (_dir, fences) = durable_fences();
    let app = flow_editor(fences);
    let bob = bob_token();
    let (status, capabilities) = flow_editor_rpc(&app, &bob, "meta/capabilities", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{capabilities}");
    assert_eq!(
        capabilities["result"]["auth"]["caller_scope"],
        caller_scope(&bob).await,
        "{capabilities}"
    );

    let standalone = crate::http_flow_editor::flow_editor_router();
    let (status, capabilities) =
        flow_editor_rpc(&standalone, &bob, "meta/capabilities", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{capabilities}");
    assert!(
        capabilities["result"]["auth"].get("caller_scope").is_none(),
        "{capabilities}"
    );

    let (status, refused) = flow_editor_rpc(
        &app,
        &bob,
        "meta/mobpacks/list",
        json!({ "expected_caller_scope": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{refused}");
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
}
