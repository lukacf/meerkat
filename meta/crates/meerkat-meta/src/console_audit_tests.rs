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

const ROOT: &str = "root@example.test";

/// A token for `email`, a person of the console test issuer.
fn person(email: &str) -> String {
    token(
        json!({ "sub": format!("sub-{email}"), "email": email, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    )
}

/// Every row `runtime`'s console audit log keeps, oldest first.
async fn stored_rows(
    runtime: &UnifiedRuntime,
) -> Result<Vec<crate::console_audit::ConsoleAuditRow>, Box<dyn std::error::Error + Send + Sync>> {
    let log = runtime
        .console_audit_log()
        .ok_or("the runtime keeps a console audit log")?;
    Ok(log.page(None, 500).await?.records)
}

/// A row as stored is bounded: at most 4 KiB encoded, at most 16 `id`
/// fields in its target, and marked as cut.
fn assert_bounded_cut_row(row: &crate::console_audit::ConsoleAuditRow) {
    let encoded = serde_json::to_string(row).expect("encode the row");
    assert!(
        encoded.len() <= 4096,
        "the row takes {} bytes",
        encoded.len()
    );
    let target = row
        .fields
        .get("target")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    assert!(
        target.keys().filter(|key| key.ends_with("_id")).count() <= 16,
        "the row keeps {} target fields",
        target.len()
    );
    assert_eq!(row.fields.get("truncated"), Some(&json!(true)));
}

/// A change a caller fills with invented identifiers, in a body near the
/// console's 2 MB JSON limit, writes one bounded row that says it left
/// things out, so the page that holds it stays small.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_two_megabyte_change_writes_a_bounded_row() -> TestResult {
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let runtime = audited_runtime(state_dir.path(), "console-audit-two-mb", None).await?;
    let router = console_router(&runtime);
    let mut params = serde_json::Map::new();
    for n in 0..6_000 {
        params.insert(format!("note{n}_id"), json!("x".repeat(300)));
    }
    let params = Value::Object(params);
    let size = params.to_string().len();
    assert!((1_500_000..2_000_000).contains(&size), "{size} bytes");
    let _answer = console_rpc_as(&router, Some(&bob()), "meta/reset", params).await;
    let rows = stored_rows(&runtime).await?;
    assert_eq!(rows.len(), 1, "one row");
    assert_bounded_cut_row(&rows[0]);
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A multipart change whose payload part carries one identifier key of
/// 100 MiB, near the route's 101 MiB body limit, writes one bounded row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_huge_multipart_change_writes_a_bounded_row() -> TestResult {
    use tower::ServiceExt;
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let runtime = audited_runtime(state_dir.path(), "console-audit-huge-multipart", None).await?;
    let router = console_router(&runtime);
    let mut params = serde_json::Map::new();
    params.insert(format!("{}_id", "k".repeat(100 * 1024 * 1024)), json!("x"));
    let payload = json!({
        "jsonrpc": "2.0",
        "id": "huge",
        "method": "meta/blob/upload",
        "params": Value::Object(params),
    })
    .to_string();
    let boundary = "console-audit-huge";
    let mut body = Vec::with_capacity(payload.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"payload\"\r\n");
    body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    body.extend_from_slice(payload.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    drop(payload);
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
                .header(header::AUTHORIZATION, format!("Bearer {}", bob()))
                .body(axum::body::Body::from(body))?,
        )
        .await?;
    let _ = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    let rows = stored_rows(&runtime).await?;
    assert_eq!(rows.len(), 1, "one row");
    assert_bounded_cut_row(&rows[0]);
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A memory provider whose remember commits, then waits until the test
/// releases it.
#[derive(Default)]
struct HeldProvider {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl crate::identity_first::AgentMemoryProvider for HeldProvider {
    async fn recall(
        &self,
        _request: crate::identity_first::AgentMemoryRecallRequest,
    ) -> Result<
        Vec<crate::identity_first::AgentMemoryRecord>,
        crate::identity_first::AgentMemoryError,
    > {
        Ok(Vec::new())
    }

    fn supports_remember(&self) -> bool {
        true
    }

    async fn remember(
        &self,
        _realm: &str,
        _identity: &crate::identity_first::AgentIdentity,
        memory: crate::identity_first::NewAgentMemory,
    ) -> Result<crate::identity_first::AgentMemoryRecord, crate::identity_first::AgentMemoryError>
    {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(crate::identity_first::AgentMemoryRecord {
            memory_id: "mem-held".to_string(),
            title: memory.title,
            body: memory.body,
            tags: memory.tags,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
    }
}

/// A change whose caller goes away still ends with its row: the change and
/// its row run on the server, so a remember the caller abandoned is recorded
/// with the outcome it reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_whose_caller_went_away_writes_its_row() -> TestResult {
    use crate::identity_first::{
        AgentAddressability, AgentIdentity, AgentMemoryConfig, AgentMemoryRuntimeInjector,
        DurabilityPolicy, DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig,
        LocalContinuityStore, LocalLeaseProvider,
    };
    let (_temp_dir, runtime) =
        super::tests::build_empty_console_test_runtime("console-audit-went-away").await?;
    let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "console-audit-went-away".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: None,
        default_timeout: None,
    }));
    identity_runtime
        .register(DurableAgentSpec {
            identity: AgentIdentity::parse("identity:went-away")?,
            profile: ProfileName::from("default"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        })
        .await;
    let provider = Arc::new(HeldProvider::default());
    identity_runtime
        .set_agent_memory(Some(AgentMemoryRuntimeInjector::new(
            provider.clone(),
            AgentMemoryConfig::default(),
        )))
        .await;
    let store: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let log = Arc::new(
        crate::console_audit::ConsoleAuditLog::over(
            store,
            crate::console_audit::ConsoleAuditLimits::default(),
        )
        .ok_or("the store keeps realm documents")?,
    );
    let router = console_json_router_with_state(ConsoleJsonState {
        runtime: Some(runtime.clone()),
        identity_runtime: Some(crate::identity_first::PublicIdentityRuntime::new(
            Arc::clone(&identity_runtime),
            runtime.clone(),
        )),
        console_audit: Some(Arc::clone(&log)),
        ..state()
    });
    let caller = bob();
    let request = tokio::spawn({
        let router = router.clone();
        async move {
            console_rpc_as(
                &router,
                Some(&caller),
                "meta/agent_memory/remember",
                json!({ "identity": "identity:went-away", "title": "Held", "body": "held" }),
            )
            .await
        }
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        provider.entered.notified(),
    )
    .await
    .map_err(|_| "the remember reached the provider")?;
    // The caller goes away while the change is under way, then the change
    // ends.
    request.abort();
    let _ = request.await;
    provider.release.notify_one();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let row = loop {
        if let Some(row) = log.page(None, 10).await?.records.into_iter().next() {
            break row;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no row was written for the abandoned change"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(row.fields["method"], json!("meta/agent_memory/remember"));
    assert_eq!(row.fields["outcome"], json!("committed"), "{row:?}");
    let _ = runtime.handle().stop().await;
    Ok(())
}

/// A row names a person's login in the decision records' form (design
/// answer A5): `sha256:` and the hex SHA-256 of the token's signed `sid`,
/// else of the token's unpadded base64url digest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_login_digest_is_the_decision_records_form() -> TestResult {
    use base64::Engine as _;
    use sha2::{Digest as _, Sha256};
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let runtime = audited_runtime(state_dir.path(), "console-audit-digest", None).await?;
    let router = console_router(&runtime);
    let mut claims = human_claims();
    claims["sid"] = json!("login-42");
    let with_sid = token(claims, Some(now_seconds() + 3600));
    let without_sid = bob();
    for caller in [&with_sid, &without_sid] {
        let _answer = console_rpc_as(
            &router,
            Some(caller),
            "meta/mob_labels/set",
            json!({ "labels": { "team": "ops" } }),
        )
        .await;
    }
    let rows = stored_rows(&runtime).await?;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows[0].fields["actor"]["login_scope_digest"],
        json!(format!("sha256:{:x}", Sha256::digest(b"login-42")))
    );
    let token_digest = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(without_sid.as_bytes()));
    assert_eq!(
        rows[1].fields["actor"]["login_scope_digest"],
        json!(format!(
            "sha256:{:x}",
            Sha256::digest(token_digest.as_bytes())
        ))
    );
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A log that would keep no row is refused when the runtime is built: its
/// count limit is at least 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zero_count_limit_is_refused_at_load() -> TestResult {
    let built = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(meerkat_mob::MobDefinition::from_toml(
                r#"
[mob]
id = "console-audit-zero"
[profiles.worker]
model = "gpt-5.5"
"#,
            )?)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .console_audit_limits(crate::ConsoleAuditLimits::new(
                0,
                std::time::Duration::from_hours(24),
            ))
            .build(),
    )
    .await;
    let refused = match built {
        Ok(runtime) => {
            let _ = runtime.shutdown().await;
            None
        }
        Err(error) => Some(error.to_string()),
    };
    assert!(
        refused
            .as_deref()
            .is_some_and(|message| message.contains("console audit")),
        "a count limit of 0 is refused: {refused:?}"
    );
    Ok(())
}

/// The audit read names a side chat's creator (the actor of its
/// `meta/fork_member` row) only to that person and to admins of an enforced
/// policy; every other reader, a listed admin of a policy not enforced
/// included, reads the row without the actor's identity. Other rows keep
/// their actors.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_audit_read_names_a_side_chat_creator_only_to_that_person_and_enforced_admins()
-> TestResult {
    let store: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let log = Arc::new(
        crate::console_audit::ConsoleAuditLog::over(
            store,
            crate::console_audit::ConsoleAuditLimits::default(),
        )
        .ok_or("the store keeps realm documents")?,
    );
    let actor = crate::console_audit::ConsoleAuditActor {
        kind: "person",
        principal: Some(BOB.to_string()),
        issuer: Some("https://trusted.meta.localhost".to_string()),
        subject: Some("oidc-sub-7".to_string()),
        assurance: None,
        login_scope_digest: Some("sha256:00".to_string()),
    };
    for (method, params) in [
        (
            "meta/fork_member",
            json!({ "source_member_id": "alpha", "agent_identity": "alpha-side" }),
        ),
        ("meta/mob_labels/set", json!({})),
    ] {
        log.record(crate::console_audit::ConsoleAuditEntry::answered(
            actor.clone(),
            method.to_string(),
            crate::console_audit::audit_target(&params),
            &json!({ "result": {} }),
        ))
        .await
        .ok_or("a row is written")?;
    }
    let router_with =
        |enabled: bool| -> Result<axum::Router, Box<dyn std::error::Error + Send + Sync>> {
            let access = AccessController::new(crate::access::AccessControlConfig {
                enabled,
                admins: vec![ROOT.to_string(), BOB.to_string()],
                ..crate::access::AccessControlConfig::default()
            })
            .map_err(|error| format!("{error:?}"))?;
            let mut console = state();
            console
                .decisions
                .auth
                .email_allowlist
                .push(ROOT.to_string());
            Ok(console_json_router_with_state(ConsoleJsonState {
                access: Some(access),
                console_audit: Some(Arc::clone(&log)),
                ..console
            }))
        };
    let read = |router: axum::Router, caller: String| async move {
        let answer = console_rpc_as(&router, Some(&caller), AUDIT, json!({})).await;
        answer["result"]["records"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("the audit log answers: {answer:#}"))
    };

    let not_enforced = router_with(false)?;
    let own = read(not_enforced.clone(), bob()).await;
    assert_eq!(
        own[0]["actor"]["principal"],
        json!(BOB),
        "the creator reads itself"
    );
    for caller in [person(ROOT), robot()] {
        let rows = read(not_enforced.clone(), caller).await;
        let fork_actor = &rows[0]["actor"];
        for field in ["principal", "issuer", "subject", "login_scope_digest"] {
            assert!(fork_actor.get(field).is_none(), "{field}: {fork_actor:#}");
        }
        assert_eq!(
            rows[1]["actor"]["principal"],
            json!(BOB),
            "other rows keep their actors"
        );
    }
    let enforced = router_with(true)?;
    let admin = read(enforced, person(ROOT)).await;
    assert_eq!(
        admin[0]["actor"]["principal"],
        json!(BOB),
        "an enforced admin reads it"
    );
    Ok(())
}

/// The console's capabilities list the audit read where the console keeps a
/// log and the caller may read it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_console_capabilities_list_the_audit_read() -> TestResult {
    let methods = |answer: &Value| -> Vec<Value> {
        answer["result"]["methods"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let open = audited_runtime(state_dir.path(), "console-audit-capabilities", None).await?;
    let router = console_router(&open);
    let listed = console_rpc_as(&router, Some(&bob()), "meta/capabilities", json!({})).await;
    assert!(methods(&listed).contains(&json!(AUDIT)), "{listed:#}");
    let _ = open.shutdown().await;

    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![BOB.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .map_err(|error| format!("{error:?}"))?;
    let enforced_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let enforced = audited_runtime(
        enforced_dir.path(),
        "console-audit-capabilities-enforced",
        Some(access),
    )
    .await?;
    let router = console_router(&enforced);
    let admin = console_rpc_as(&router, Some(&bob()), "meta/capabilities", json!({})).await;
    assert!(methods(&admin).contains(&json!(AUDIT)), "{admin:#}");
    let other = console_rpc_as(&router, Some(&robot()), "meta/capabilities", json!({})).await;
    assert!(
        !methods(&other).contains(&json!(AUDIT)),
        "a caller without audit.view is not offered the read: {other:#}"
    );
    let _ = enforced.shutdown().await;
    Ok(())
}

/// Every MCP app tool call writes a row, whatever its tool does: the console
/// cannot tell whether a tool changes anything. A call the console refuses
/// is recorded as refused, with its HTTP status and kind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_mcp_app_tool_call_writes_an_audit_row() -> TestResult {
    use tower::ServiceExt;
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let runtime = audited_runtime(state_dir.path(), "console-audit-call-tool", None).await?;
    let router = console_router(&runtime);
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/mcp-apps/call-tool")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {}", bob()))
                .body(axum::body::Body::from(
                    json!({
                        "identity": "worker-1",
                        "sessionId": meerkat_core::types::SessionId::new().to_string(),
                        "toolCallId": "call-1",
                        "name": "file_ticket",
                        "arguments": {},
                    })
                    .to_string(),
                ))?,
        )
        .await?;
    let status = response.status();
    let rows = stored_rows(&runtime).await?;
    assert_eq!(rows.len(), 1, "the call writes one row ({status})");
    let row = &rows[0].fields;
    assert_eq!(row["method"], json!("/console/mcp-apps/call-tool"));
    assert_eq!(row["target"]["tool"], json!("file_ticket"));
    assert_eq!(row["target"]["identity"], json!("worker-1"));
    assert_eq!(row["outcome"], json!("refused"));
    assert_eq!(row["code"], json!(status.as_u16()));
    assert_eq!(row["kind"], json!("mcp_app_unavailable"));
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A Flow Editor change (here a validation through `rkat`) writes a row
/// naming the flow; a validation that runs nothing on the host and a read
/// write none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flow_editor_change_writes_an_audit_row() -> TestResult {
    use tower::ServiceExt;
    let state_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
    let runtime = audited_runtime(state_dir.path(), "console-audit-flow-editor", None).await?;
    let router = runtime.build_reference_app_router(state().decisions);
    for (method, params) in [
        (
            "meta/mobpacks/validate",
            json!({ "id": "flow-1", "rkat_validate": true }),
        ),
        ("meta/mobpacks/validate", json!({ "id": "flow-1" })),
        ("meta/mobpacks/list", json!({})),
    ] {
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/flow-editor/rpc")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {}", bob()))
                    .body(axum::body::Body::from(
                        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
                            .to_string(),
                    ))?,
            )
            .await?;
        let _ = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    }
    let rows = stored_rows(&runtime).await?;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].fields["method"], json!("meta/mobpacks/validate"));
    assert_eq!(rows[0].fields["target"]["id"], json!("flow-1"));
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A slow audit store does not hold a change's answer: the console waits
/// for the row a bounded time, then answers and lets the row land.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_audit_store_holds_an_answer_at_most_its_wait() -> TestResult {
    let store = Arc::new(
        crate::console_audit::test_support::ListingStore::holding_writes_for(
            std::time::Duration::from_secs(8),
        ),
    );
    let log = Arc::new(
        crate::console_audit::ConsoleAuditLog::over(
            store as Arc<dyn crate::runtime::PersistentMetadataStore>,
            crate::console_audit::ConsoleAuditLimits::default(),
        )
        .ok_or("the store keeps realm documents")?,
    );
    let router = console_json_router_with_state(ConsoleJsonState {
        console_audit: Some(Arc::clone(&log)),
        ..state()
    });
    let started = std::time::Instant::now();
    let _answer = console_rpc_as(
        &router,
        Some(&bob()),
        "meta/reset",
        json!({ "identity": "nobody" }),
    )
    .await;
    let waited = started.elapsed();
    assert!(
        waited < std::time::Duration::from_secs(5),
        "the answer waited {waited:?} for its row"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if !log.page(None, 10).await?.records.is_empty() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the row lands");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(())
}
