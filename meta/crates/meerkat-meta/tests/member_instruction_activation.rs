//! A host-activated member instruction is forward-only and durable.
//!
//! `meta/member_activate_instruction` appends one keyed activation to the
//! member's current session through Meerkat's member-level door
//! (`MobHandle::activate_member_instruction`). Booted on durable state the way
//! a deployment boots (persistent mob storage, a SQLite session store, a
//! durable continuity store), the activation must still be the member's
//! effective instruction after a restart, and re-applying it then is a typed
//! `duplicate` read from durable state. The host verb is not reachable from
//! the member: its resolved agent tools carry no instruction-activation tool.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::identity_first::orchestrator::RestoreOutcome;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, AgentRuntimeServices, ContinuityStore, DurabilityPolicy,
    DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider, MobSessionBridge, MutableRosterProvider,
};
use meerkat_meta::mob_composition_manifest::persistent_mob_storage;
use meerkat_meta::{DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec};
use meerkat_mob::{MobDefinition, ProfileName};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

const IDENTITY: &str = "identity:member-1";
const BODY: &str = "Use the tools you were granted; your skills changed in activation 195.";

fn definition() -> MobDefinition {
    MobDefinition::from_toml(
        r#"
[mob]
id = "member-instruction-activation"

[profiles.parent]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.parent.tools]
comms = true
"#,
    )
    .expect("parse the definition")
}

fn spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(IDENTITY).unwrap(),
        profile: ProfileName::from("parent"),
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
    }
}

/// Answers every turn; no turn runs here, but the runtime needs a client.
#[derive(Clone, Default)]
struct OkClient;

impl meerkat_client::LlmClient for OkClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

struct Booted {
    unified: meerkat_meta::UnifiedRuntime,
    session_id: meerkat_core::types::SessionId,
}

/// Boot on durable state, activated through
/// `install_and_bootstrap_identity_first_context`.
async fn boot(state_path: &std::path::Path) -> Booted {
    std::fs::create_dir_all(state_path).expect("state root");
    let (storage, provenance) = persistent_mob_storage(state_path.join("mob.sqlite3"))
        .expect("open persistent mob storage");
    let spec_bootstrap = MobBootstrapSpec::persistent(
        definition(),
        storage,
        provenance,
        state_path.to_path_buf(),
        16,
    )
    .await
    .expect("compose persistent Meta stores")
    // Creates no agent child mob. It restarts in this process, which the
    // durable child-mob default refuses as a duplicate owner; explicit
    // ProcessBound keeps this test's cold-recovery oracle.
    .with_child_mob_storage(meerkat_meta::ChildMobStorage::ProcessBound)
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(OkClient)),
    });
    let mut unified = meerkat_meta::UnifiedRuntime::bootstrap(
        spec_bootstrap,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "member-instruction-activation".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("bootstrap the UnifiedRuntime");
    let bridge = Arc::new(MobSessionBridge::with_session_service(
        unified.mob_handle(),
        unified
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("the persistent runtime has a session service"),
    ));
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.sqlite3"))
            .expect("continuity store"),
    );
    let identity_runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: store as Arc<dyn ContinuityStore>,
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "member-instruction-activation".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(unified.mob_handle())),
    );
    let roster = vec![spec()];
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        Arc::clone(&identity_runtime),
        Arc::new(MutableRosterProvider::new(roster.clone())),
        None,
        None,
        Some(definition()),
    ));
    let result = unified
        .install_and_bootstrap_identity_first_context(context, &roster)
        .await
        .expect("activate and restore the identity-first runtime");
    let session_id = match result
        .outcomes
        .get(&AgentIdentity::parse(IDENTITY).unwrap())
    {
        Some(RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. }) => {
            binding
                .session_id
                .clone()
                .expect("Core binds the identity's member to a session")
        }
        other => panic!("{IDENTITY} did not materialize: {other:?}"),
    };
    Booted {
        unified,
        session_id,
    }
}

/// One JSON-RPC call through the gateway's real dispatcher.
async fn rpc(booted: &Booted, method: &str, params: Value) -> Value {
    let ctx = meerkat_meta::rpc::IdentityFirstContext {
        // The installed runtime's owning view, from the real composition.
        runtime: booted
            .unified
            .identity_runtime()
            .expect("identity runtime installed"),
        roster_provider: Arc::new(MutableRosterProvider::new(vec![spec()])),
        topology_provider: None,
        customizer: None,
        agent_memory_provider: None,
        mob_definition: Some(definition()),
        transcript_edit_service: None,
        compaction_floors: None,
    };
    let raw = meerkat_meta::handle_unified_rpc_json(
        &booted.unified,
        &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string(),
        std::time::Duration::from_mins(1),
        None,
        Some(&ctx),
    )
    .await;
    serde_json::from_str(&raw).expect("json-rpc response")
}

fn activation() -> Value {
    json!({
        "revision": {
            "namespace": "host",
            "key": "persona",
            "revision_id": "r195",
            "content_sha256": meerkat_core::InstructionContentDigest::for_body(BODY).as_str(),
        },
        "activation_id": "a195",
        "expectation": {"kind": "absent"},
        "body": BODY,
    })
}

async fn activation_records(booted: &Booted) -> Vec<Value> {
    let page = rpc(
        booted,
        "meta/member_instruction_activations",
        json!({"identity": IDENTITY, "namespace": "host", "key": "persona"}),
    )
    .await;
    assert!(page["error"].is_null(), "{page:#?}");
    page["result"]["page"]["records"]
        .as_array()
        .expect("records array")
        .clone()
}

async fn transcript_mentions_body(booted: &Booted) -> bool {
    let history = booted
        .unified
        .mob_runtime()
        .session_service()
        .expect("session service")
        .read_history(
            &booted.session_id,
            meerkat_core::service::SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect("read the member's history");
    history
        .messages
        .iter()
        .any(|message| serde_json::to_string(message).unwrap().contains(BODY))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_activated_member_instruction_survives_a_restart() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");

    // Boot 1: the host activates the instruction on the member's session.
    let booted = boot(&state_path).await;
    let response = rpc(
        &booted,
        "meta/member_activate_instruction",
        json!({"identity": IDENTITY, "activation": activation()}),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    assert_eq!(
        response["result"]["receipt"]["disposition"],
        json!("applied"),
        "{response:#?}"
    );
    assert_eq!(activation_records(&booted).await.len(), 1);
    assert!(transcript_mentions_body(&booted).await);
    let session_id = booted.session_id.clone();

    // The member cannot reach the host verb: no resolved agent tool is an
    // instruction-activation tool.
    let tools = meerkat_meta::mob_handle_runtime::resolved_tools_for_session(
        booted.unified.mob_runtime().session_service(),
        IDENTITY,
        session_id.clone(),
    )
    .await
    .expect("resolve the member's tools")
    .tools;
    assert!(
        !tools.iter().any(|tool| tool.contains("instruction")),
        "the host verb must not be an agent tool: {tools:?}"
    );
    booted.unified.shutdown().await;
    drop(booted);

    // Boot 2: the same durable state. The member resumes its session, and the
    // activation is still its effective instruction.
    let booted = boot(&state_path).await;
    assert_eq!(
        booted.session_id, session_id,
        "the member resumes its session"
    );
    let records = activation_records(&booted).await;
    assert_eq!(records.len(), 1, "the activation is durable: {records:?}");
    assert_eq!(records[0]["identity"]["activation_id"], json!("a195"));
    assert!(
        transcript_mentions_body(&booted).await,
        "the resumed transcript still carries the instruction"
    );
    let again = rpc(
        &booted,
        "meta/member_activate_instruction",
        json!({"identity": IDENTITY, "activation": activation()}),
    )
    .await;
    assert!(again["error"].is_null(), "{again:#?}");
    assert_eq!(
        again["result"]["receipt"]["disposition"],
        json!("duplicate"),
        "re-applying after the restart is a duplicate from durable state: {again:#?}"
    );
    booted.unified.shutdown().await;
}
