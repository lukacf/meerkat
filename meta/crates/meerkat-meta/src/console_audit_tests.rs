//! The console audit log: one row per console change, naming who made it
//! and how it ended, kept in the realm's metadata store across restarts and
//! read with `meta/console/audit` under the `audit.view` action. Both audit
//! reads say how long their records are kept.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_mob::SpawnMemberSpec;
use serde_json::{Value, json};

use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{console_rpc_as, human_claims, now_seconds, token};
use super::*;
use crate::unified_runtime::{UnifiedRuntime, UnifiedRuntimeBuilder};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const AUDIT: &str = "meta/console/audit";
const ROBOT: &str = "svc:robot";

/// A console runtime as a gateway composes one over the state directory
/// `state_dir`, with the access controller `access` when one is given.
async fn audited_runtime(
    state_dir: &std::path::Path,
    mob_id: &str,
    access: Option<AccessController>,
) -> Result<UnifiedRuntime, Box<dyn std::error::Error + Send + Sync>> {
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"
[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#
        ))?)
        .persistent_state(state_dir)
        .comms(true)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()));
    if let Some(access) = access {
        builder = builder.access_controller(access);
    }
    Ok(Box::pin(builder.build())
        .await
        .map_err(|error| error.to_string())?)
}

/// The runtime's console router, signing callers in with the console test
/// issuer's tokens.
fn console_router(runtime: &UnifiedRuntime) -> axum::Router {
    runtime.build_console_json_router(state().decisions)
}

async fn spawn_workers(runtime: &UnifiedRuntime, aliases: &[&str]) -> TestResult {
    for alias in aliases {
        runtime
            .mob_handle()
            .spawn_spec(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                (*alias).to_string(),
                None,
                None,
                None,
            ))
            .await?;
    }
    Ok(())
}

fn bob() -> String {
    token(human_claims(), Some(now_seconds() + 3600))
}

fn robot() -> String {
    token(json!({ "email": ROBOT }), Some(now_seconds() + 3600))
}

/// Every row of the log, oldest first, as `caller` reads it.
async fn audit_rows(router: &axum::Router, caller: &str) -> Vec<Value> {
    let read = console_rpc_as(router, Some(caller), AUDIT, json!({ "limit": 500 })).await;
    assert!(read["error"].is_null(), "the audit log answers: {read:#}");
    read["result"]["records"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the audit read lists rows: {read:#}"))
}

/// The outcome a row records for `answer`: `committed` for a result,
/// `failed` for an owner failure (whether anything changed is not known),
/// and `refused` for every other error.
fn outcome_of(answer: &Value) -> &'static str {
    match answer["error"]["code"].as_i64() {
        None if answer["error"].is_null() => "committed",
        Some(-32603 | -32000) => "failed",
        _ => "refused",
    }
}

/// A one-file `meta/blob/upload` on the multipart route, as `caller`: the
/// JSON-RPC answer.
async fn upload_blob(router: &axum::Router, caller: &str) -> Value {
    use tower::ServiceExt;
    let boundary = "console-audit-boundary";
    let payload = json!({
        "jsonrpc": "2.0",
        "id": "upload-1",
        "method": "meta/blob/upload",
        "params": {
            "upload": {
                "type": "image_upload",
                "upload_id": "upload-1",
                "media_type": "image/png"
            }
        }
    });
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"payload\"\r\n");
    body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    body.extend_from_slice(payload.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file:upload-1\"; filename=\"tiny.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(b"tiny-png");
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/rpc/multipart")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {caller}"))
                .body(axum::body::Body::from(body))
                .expect("multipart request"),
        )
        .await
        .expect("multipart response");
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("multipart body");
    serde_json::from_slice(&body).expect("multipart answer")
}

/// Each console change writes one row: who made it (kind, principal,
/// assurance and a digest of the login, never the token), the method, its
/// target and how it ended, a refusal with its code. A label edit, a
/// retire, a reset, a `delete_identity`, a forget, an access edit, a session
/// revocation and a blob upload each write one, whatever their owner
/// answered, and reading the log writes none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_mutations_write_audit_rows() -> TestResult {
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let runtime = audited_runtime(state_dir.path(), "console-audit-rows", None).await?;
    spawn_workers(&runtime, &["alpha", "beta"]).await?;
    let router = console_router(&runtime);
    let bob = bob();

    let changes = [
        (
            "meta/mob_labels/set",
            json!({ "labels": { "team": "ops" } }),
        ),
        ("meta/retire_member", json!({ "member_id": "alpha" })),
        ("meta/reset", json!({ "identity": "beta" })),
        ("meta/delete_identity", json!({ "identity": "beta" })),
        (
            "meta/agent_memory/forget",
            json!({ "realm": "meta", "identity": "beta", "memory_id": "mem-1" }),
        ),
        ("meta/access/set", json!({ "config": { "enabled": false } })),
        (
            crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
            json!({ "principal": ROBOT }),
        ),
    ];
    let mut answers = Vec::new();
    for (method, params) in &changes {
        answers.push((
            *method,
            console_rpc_as(&router, Some(&bob), method, params.clone()).await,
        ));
    }
    answers.push(("meta/blob/upload", upload_blob(&router, &bob).await));
    assert!(
        answers[0].1["error"].is_null(),
        "the label edit commits: {:#}",
        answers[0].1
    );
    assert!(
        answers[7].1["error"].is_null(),
        "the blob upload commits: {:#}",
        answers[7].1
    );

    let rows = audit_rows(&router, &bob).await;
    assert_eq!(
        rows.len(),
        answers.len(),
        "one row per change, and none for the read: {rows:#?}"
    );
    for (index, ((method, answer), row)) in answers.iter().zip(&rows).enumerate() {
        assert_eq!(row["seq"], json!(index + 1), "{row:#}");
        assert_eq!(row["method"], json!(method), "{row:#}");
        assert_eq!(row["actor"]["kind"], json!("person"), "{row:#}");
        assert_eq!(row["actor"]["principal"], json!(BOB), "{row:#}");
        assert_eq!(row["actor"]["subject"], json!("oidc-sub-7"), "{row:#}");
        assert_eq!(
            row["actor"]["assurance"],
            json!("person_authenticated"),
            "{row:#}"
        );
        assert!(
            row["actor"]["login_scope_digest"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:")),
            "{row:#}"
        );
        assert_eq!(
            row["outcome"],
            json!(outcome_of(answer)),
            "{method}: {answer:#} {row:#}"
        );
        if !answer["error"].is_null() {
            assert_eq!(row["code"], answer["error"]["code"], "{row:#}");
        }
        assert!(row["at_ms"].as_u64().is_some_and(|at| at > 0), "{row:#}");
    }
    assert_eq!(rows[1]["target"]["member_id"], json!("alpha"));
    assert_eq!(rows[4]["target"]["memory_id"], json!("mem-1"));
    assert_eq!(rows[6]["target"]["principal"], json!(ROBOT));
    assert_eq!(
        rows[7]["receipt"]["blob_id"],
        answers[7].1["result"]["blob_id"]
    );
    let read = console_rpc_as(&router, Some(&bob), AUDIT, json!({})).await;
    assert!(
        !read.to_string().contains(&bob),
        "the log holds no token bytes"
    );
    assert_eq!(read["result"]["latest_seq"], json!(answers.len()));
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A console decide writes a row that cites the decision: the gating entry
/// it decided and its action, with the outcome the owner recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_decide_writes_audit_row_with_decision_ref() -> TestResult {
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let runtime = audited_runtime(state_dir.path(), "console-audit-decide", None).await?;
    let gated = runtime
        .evaluate_gating_action(crate::runtime::GatingEvaluateRequest {
            action: "deploy".to_string(),
            actor_id: "alpha".to_string(),
            risk_tier: crate::runtime::GatingRiskTier::R3,
            rationale: None,
            requested_approver: None,
            approval_recipient: None,
            approval_channel: None,
            approval_timeout_ms: Some(600_000),
            entity: None,
            topic: None,
        })
        .await;
    let pending_id = gated
        .pending_id
        .clone()
        .ok_or("an R3 action waits for approval")?;
    let router = console_router(&runtime);
    let bob = bob();
    let decided = console_rpc_as(
        &router,
        Some(&bob),
        "meta/gating/decide",
        json!({ "pending_id": pending_id, "decision": "approve" }),
    )
    .await;
    assert!(
        decided["error"].is_null(),
        "the decide commits: {decided:#}"
    );

    let rows = audit_rows(&router, &bob).await;
    let row = rows
        .iter()
        .find(|row| row["method"] == json!("meta/gating/decide"))
        .unwrap_or_else(|| panic!("the decide wrote a row: {rows:#?}"));
    assert_eq!(row["outcome"], json!("committed"), "{row:#}");
    assert_eq!(row["actor"]["principal"], json!(BOB), "{row:#}");
    assert_eq!(row["target"]["pending_id"], json!(pending_id), "{row:#}");
    assert_eq!(row["target"]["decision"], json!("approve"), "{row:#}");
    assert_eq!(row["receipt"]["pending_id"], json!(pending_id), "{row:#}");
    assert_eq!(
        row["receipt"]["action_id"],
        json!(gated.action_id),
        "{row:#}"
    );
    let _ = runtime.shutdown().await;
    Ok(())
}

/// Reading the log needs `audit.view`: under an enforced access policy a
/// caller without it is refused, typed, and a caller a rule grants it to
/// reads the rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_read_needs_audit_view() -> TestResult {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![
            AccessRule {
                id: "bob-views-agents".to_string(),
                subjects: vec![BOB.to_string()],
                actions: vec![ACTION_AGENT_VIEW.to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "the-robot-reads-the-audit".to_string(),
                subjects: vec![ROBOT.to_string()],
                actions: vec!["audit.view".to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .map_err(|error| format!("{error:?}"))?;
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let runtime = audited_runtime(state_dir.path(), "console-audit-read", Some(access)).await?;
    let router = console_router(&runtime);

    let refused = console_rpc_as(&router, Some(&bob()), AUDIT, json!({})).await;
    assert_eq!(
        refused["error"]["code"],
        json!(ACCESS_DENIED_RPC_CODE),
        "{refused:#}"
    );
    assert_eq!(refused["error"]["data"]["action"], json!("audit.view"));
    let allowed = console_rpc_as(&router, Some(&robot()), AUDIT, json!({})).await;
    assert!(allowed["error"].is_null(), "{allowed:#}");
    assert!(allowed["result"]["records"].is_array(), "{allowed:#}");
    let _ = runtime.shutdown().await;
    Ok(())
}

/// The log is durable: a runtime started again over the same state
/// directory reads the rows the earlier one wrote, and numbers its own
/// after them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_audit_survives_restart() -> TestResult {
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let bob = bob();
    let first = audited_runtime(state_dir.path(), "console-audit-restart", None).await?;
    let answer = console_rpc_as(
        &console_router(&first),
        Some(&bob),
        "meta/mob_labels/set",
        json!({ "labels": { "team": "ops" } }),
    )
    .await;
    assert!(answer["error"].is_null(), "{answer:#}");
    let _ = first.shutdown().await;
    drop(first);

    let second = audited_runtime(state_dir.path(), "console-audit-restart", None).await?;
    let router = console_router(&second);
    let answer = console_rpc_as(
        &router,
        Some(&bob),
        "meta/mob_labels/set",
        json!({ "labels": { "team": "review" } }),
    )
    .await;
    assert!(answer["error"].is_null(), "{answer:#}");
    let rows = audit_rows(&router, &bob).await;
    assert_eq!(
        rows.iter()
            .map(|row| row["seq"].clone())
            .collect::<Vec<_>>(),
        vec![json!(1), json!(2)],
        "the earlier row survived and the next one follows it: {rows:#?}"
    );
    let _ = second.shutdown().await;
    Ok(())
}

/// The two audit reads say how long their records are kept. The console
/// audit log lasts as the metadata store the runtime opened keeps its
/// documents: `durable` over a state directory, `process` in memory. The
/// gating audit is `durable` only where the runtime itself keeps the gating
/// state, which a runtime built without `keep_gating_state` does not, over a
/// state directory too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_reads_report_their_retention() -> TestResult {
    let bob = bob();
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let persistent = audited_runtime(state_dir.path(), "console-audit-durable", None).await?;
    let router = console_router(&persistent);
    let audit = console_rpc_as(&router, Some(&bob), AUDIT, json!({})).await;
    assert_eq!(audit["result"]["retention"], json!("durable"), "{audit:#}");
    let gating = console_rpc_as(&router, Some(&bob), "meta/gating/audit", json!({})).await;
    assert_eq!(
        gating["result"]["retention"],
        json!("process"),
        "{gating:#}"
    );
    let _ = persistent.shutdown().await;

    let in_memory = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(meerkat_mob::MobDefinition::from_toml(
                r#"
[mob]
id = "console-audit-process"
[profiles.worker]
model = "gpt-5.5"
"#,
            )?)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .map_err(|error| error.to_string())?;
    let router = console_router(&in_memory);
    let audit = console_rpc_as(&router, Some(&bob), AUDIT, json!({})).await;
    assert_eq!(audit["result"]["retention"], json!("process"), "{audit:#}");
    let gating = console_rpc_as(&router, Some(&bob), "meta/gating/audit", json!({})).await;
    assert_eq!(
        gating["result"]["retention"],
        json!("process"),
        "{gating:#}"
    );
    let _ = in_memory.shutdown().await;
    Ok(())
}
