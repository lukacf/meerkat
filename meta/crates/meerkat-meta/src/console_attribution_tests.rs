//! Who made a change, kept on the owner's own record where the owner keeps
//! one: a memory written from the console names the person who wrote it,
//! an access edit names its author and time, and a session revocation names
//! who revoked.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{
    console_rpc_as, durable_fences, human_claims, now_seconds, token,
};
use super::*;
use crate::identity_first::{
    AgentAddressability, AgentIdentity, AgentMemoryConfig, AgentMemoryRuntimeInjector,
    DurabilityPolicy, DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider,
};
use crate::memory::SqliteAgentMemoryStore;
use crate::memory::capabilities::MemoryPanelStore as _;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const ROBOT: &str = "svc:robot";

fn bob() -> String {
    token(human_claims(), Some(now_seconds() + 3600))
}

/// A console change through the runtime dispatch, made by `principal`.
async fn console_change_by(
    runtime: &MobRuntime,
    identity_runtime: &Arc<IdentityRuntime>,
    principal: &str,
    method: &str,
    params: Value,
) -> Value {
    Box::pin(handle_console_runtime_rpc_with_visibility(
        runtime,
        None,
        None,
        None,
        None,
        None,
        Some(Arc::clone(identity_runtime)),
        None,
        None,
        &crate::console_aggregator::AllowAllConsoleVisibilityPolicy,
        JsonRpcRequest {
            jsonrpc: crate::rpc::JSONRPC_VERSION.to_string(),
            id: Some(json!("attribution")),
            method: method.to_string(),
            params,
        },
        true,
        false,
        Some(principal),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        &ConsoleSendGovernance::default(),
        &crate::topology_control::TopologyApplyScope::Anonymous,
    ))
    .await
}

/// A memory a person writes from the console names them: the record's
/// author is the operator, with the person's principal, on a remember and
/// on the update that supersedes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_memory_write_records_person() -> TestResult {
    let (_temp_dir, runtime) =
        super::tests::build_empty_console_test_runtime("console-memory-person").await?;
    let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "console-memory-person".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: None,
        default_timeout: None,
    }));
    let identity = AgentIdentity::parse("identity:memory-person")?;
    identity_runtime
        .register(DurableAgentSpec {
            identity: identity.clone(),
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
    let memory_dir = tempfile::tempdir()?;
    let memory_store = Arc::new(SqliteAgentMemoryStore::open(memory_dir.path())?);
    identity_runtime
        .set_agent_memory(Some(AgentMemoryRuntimeInjector::new(
            memory_store.clone(),
            AgentMemoryConfig::default(),
        )))
        .await;

    let remembered = console_change_by(
        &runtime,
        &identity_runtime,
        BOB,
        "meta/agent_memory/remember",
        json!({
            "identity": "identity:memory-person",
            "title": "Deploy window",
            "body": "Deploys run on Tuesdays.",
        }),
    )
    .await;
    assert!(remembered["error"].is_null(), "{remembered:#}");
    let memory_id = remembered["result"]["memory_id"]
        .as_str()
        .ok_or("the remember answers its memory id")?
        .to_string();
    let author_of = |memory_id: String| {
        let memory_store = Arc::clone(&memory_store);
        async move {
            let record = memory_store
                .record_by_id("default", &memory_id)
                .await
                .expect("read the record")
                .expect("the record exists");
            serde_json::to_value(&record.provenance.author).expect("encode the author")
        }
    };
    let person = json!({ "author": "operator", "principal": BOB });
    assert_eq!(author_of(memory_id.clone()).await, person);

    let updated = console_change_by(
        &runtime,
        &identity_runtime,
        BOB,
        "meta/agent_memory/update",
        json!({
            "identity": "identity:memory-person",
            "memory_id": memory_id,
            "title": "Deploy window",
            "body": "Deploys run on Wednesdays.",
        }),
    )
    .await;
    assert!(updated["error"].is_null(), "{updated:#}");
    let successor = updated["result"]["memory_id"]
        .as_str()
        .ok_or("the update answers the new memory id")?
        .to_string();
    assert_eq!(author_of(successor).await, person);
    let _ = runtime.handle().stop().await;
    Ok(())
}

/// A session revocation names who revoked, in its answer and in the fence
/// record, which keeps it across a reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_records_the_revoker() -> TestResult {
    let (dir, fences) = durable_fences();
    let router = console_json_router_with_state(ConsoleJsonState {
        session_fences: fences,
        ..state()
    });
    let revoked = console_rpc_as(
        &router,
        Some(&bob()),
        crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
        json!({ "principal": ROBOT }),
    )
    .await;
    assert!(revoked["error"].is_null(), "{revoked:#}");
    assert_eq!(revoked["result"]["principal"], json!(ROBOT), "{revoked:#}");
    assert_eq!(revoked["result"]["revoked_by"], json!(BOB), "{revoked:#}");

    let reopened = crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(dir.path())?;
    let fence = reopened
        .fences()
        .into_iter()
        .find(|fence| fence.principal == ROBOT)
        .ok_or("the fence survived the reopen")?;
    assert_eq!(
        crate::rpc::console_session_fence_json(&fence)["revoked_by"],
        json!(BOB)
    );
    Ok(())
}

/// An access edit records its author and time: `meta/access/get` names who
/// made the last edit, when, and at which revision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn access_edit_records_author_and_time() -> TestResult {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![BOB.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .map_err(|error| format!("{error:?}"))?;
    let router = console_json_router_with_state(ConsoleJsonState {
        access: Some(access),
        ..state()
    });
    let bob = bob();
    let before = now_seconds() * 1000;
    let edited = console_rpc_as(
        &router,
        Some(&bob),
        "meta/access/rules/upsert",
        json!({
            "rule": {
                "id": "ops-view",
                "subjects": [ROBOT],
                "actions": ["agent.view"],
            }
        }),
    )
    .await;
    assert!(edited["error"].is_null(), "{edited:#}");
    let read = console_rpc_as(&router, Some(&bob), "meta/access/get", json!({})).await;
    assert!(read["error"].is_null(), "{read:#}");
    let last_edit = &read["result"]["last_edit"];
    assert_eq!(last_edit["principal"], json!(BOB), "{read:#}");
    assert_eq!(
        last_edit["revision"], read["result"]["revision"],
        "{read:#}"
    );
    assert!(
        last_edit["at_ms"].as_u64().is_some_and(|at| at >= before),
        "{read:#}"
    );
    Ok(())
}
