//! Meta's operator-instruction host verbs over Core's mob.
//!
//! `meta/set_instruction`, `meta/revoke_instruction` and
//! `meta/instruction_status` map Meta's names onto Core's mob handle
//! (`MobHandle::set_operator_instruction`, `revoke_operator_instruction` and
//! `operator_instruction_status`) and keep no state of their own. A member
//! target names a Meta identity, which reaches Core as that identity's roster
//! member; a profile target names the mob definition's profile; the status
//! read names every member by its Meta identity again. They are host verbs:
//! the gateway lists them, and no agent tool sets, revokes or reads an
//! operator instruction.
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
use meerkat_mob::{MobDefinition, OperatorInstructionTarget, ProfileName};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

const IDENTITY: &str = "identity:member-1";
const PROFILE: &str = "parent";
const PERSONA: &str = "Answer as the release reviewer.";
const TONE: &str = "Keep every answer short.";

/// The three host verbs.
const INSTRUCTION_VERBS: [&str; 3] = [
    "meta/set_instruction",
    "meta/revoke_instruction",
    "meta/instruction_status",
];

fn definition() -> MobDefinition {
    MobDefinition::from_toml(
        r#"
[mob]
id = "operator-instructions"

[profiles.parent]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.parent.tools]
builtins = true
comms = true
mob = true
"#,
    )
    .expect("parse the definition")
}

fn spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(IDENTITY).unwrap(),
        profile: ProfileName::from(PROFILE),
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
/// `install_and_bootstrap_identity_first_context`, with the member
/// materialized once the roster pass seated it.
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
                namespace: "operator-instructions".to_string(),
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
            runtime_instance_id: "operator-instructions".to_string(),
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
        other => panic!("{IDENTITY} was not seated: {other:?}"),
    };
    identity_runtime
        .materialize(&AgentIdentity::parse(IDENTITY).unwrap())
        .await
        .expect("Core materializes the member");
    Booted {
        unified,
        session_id,
    }
}

/// One JSON-RPC call through the gateway's real dispatcher.
async fn rpc(booted: &Booted, method: &str, params: Value) -> Value {
    let ctx = meerkat_meta::rpc::IdentityFirstContext {
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

/// The result of a call that must succeed.
fn result_of(response: &Value) -> &Value {
    assert!(response["error"].is_null(), "{response:#?}");
    &response["result"]
}

/// A record of a status read by its key.
fn record<'a>(status: &'a Value, key: &str) -> &'a Value {
    status["desired"]
        .as_array()
        .expect("desired records")
        .iter()
        .find(|record| record["key"] == json!(key))
        .unwrap_or_else(|| panic!("no record for {key}: {status:#?}"))
}

/// A member write names a Meta identity and reaches Core as that identity's
/// roster member; a profile write names the definition's profile. Core's own
/// read shows both targets in its spelling, and Meta's read names the member
/// by its Meta identity again. A revoke keeps the record's target, and a
/// stale revision is refused with its closed code.
#[tokio::test(flavor = "multi_thread")]
async fn meta_set_instruction_maps_identity_and_profile_to_core() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let booted = boot(&temp.path().join("state")).await;

    let member_write = rpc(
        &booted,
        "meta/set_instruction",
        json!({
            "namespace": "host",
            "key": "persona",
            "revision": 1,
            "body": PERSONA,
            "target": {"kind": "member", "identity": IDENTITY},
        }),
    )
    .await;
    let receipt = result_of(&member_write);
    assert_eq!(receipt["disposition"], json!("committed"), "{receipt:#?}");
    assert_eq!(receipt["revision"], json!(1));
    assert_eq!(
        receipt["matched_members"],
        json!(1),
        "the identity's member is selected: {receipt:#?}"
    );

    let profile_write = rpc(
        &booted,
        "meta/set_instruction",
        json!({
            "namespace": "host",
            "key": "tone",
            "revision": 1,
            "body": TONE,
            "content_sha256": meerkat_core::InstructionContentDigest::for_body(TONE).as_str(),
            "target": {"kind": "profile", "profile": PROFILE},
        }),
    )
    .await;
    let receipt = result_of(&profile_write);
    assert_eq!(receipt["disposition"], json!("committed"), "{receipt:#?}");
    assert_eq!(
        receipt["matched_members"],
        json!(1),
        "the profile's member is selected: {receipt:#?}"
    );

    // Core's own read: the member target is the identity's roster member.
    let core = booted
        .unified
        .mob_handle()
        .operator_instruction_status(meerkat_mob::OperatorInstructionStatusQuery::default())
        .await
        .expect("Core reads its operator instructions");
    let targets: BTreeMap<String, OperatorInstructionTarget> = core
        .desired
        .iter()
        .map(|record| (record.slot.key.to_string(), record.target.clone()))
        .collect();
    assert_eq!(
        targets.get("persona"),
        Some(&OperatorInstructionTarget::Member {
            agent_identity: meerkat_meta::member_comms_id::roster_member_id_for_identity(IDENTITY),
            include_forks: false,
        }),
        "{targets:#?}"
    );
    assert_eq!(
        targets.get("tone"),
        Some(&OperatorInstructionTarget::Profile {
            profile: ProfileName::from(PROFILE),
        }),
        "{targets:#?}"
    );

    // Meta's read names the member by its Meta identity, in the records and
    // in the member rows.
    let status = rpc(
        &booted,
        "meta/instruction_status",
        json!({"identity": IDENTITY}),
    )
    .await;
    let status = result_of(&status);
    assert_eq!(
        record(status, "persona")["target"],
        json!({"kind": "member", "identity": IDENTITY}),
        "{status:#?}"
    );
    assert_eq!(
        record(status, "tone")["target"],
        json!({"kind": "profile", "profile": PROFILE}),
        "{status:#?}"
    );
    let members = status["members"].as_array().expect("member rows");
    assert_eq!(members.len(), 1, "{status:#?}");
    assert_eq!(members[0]["identity"], json!(IDENTITY), "{status:#?}");
    assert_eq!(members[0]["profile"], json!(PROFILE), "{status:#?}");
    assert!(
        members[0].get("agent_identity").is_none(),
        "a member is named by its Meta identity only: {status:#?}"
    );
    let slots: Vec<&Value> = members[0]["slots"]
        .as_array()
        .expect("slot rows")
        .iter()
        .map(|slot| &slot["key"])
        .collect();
    assert_eq!(slots, [&json!("persona"), &json!("tone")], "{status:#?}");

    // A profile filter answers the same member.
    let by_profile = rpc(
        &booted,
        "meta/instruction_status",
        json!({"profile": PROFILE, "namespace": "host", "key": "tone"}),
    )
    .await;
    let by_profile = result_of(&by_profile);
    assert_eq!(
        by_profile["members"][0]["identity"],
        json!(IDENTITY),
        "{by_profile:#?}"
    );
    assert_eq!(
        by_profile["desired"].as_array().map(Vec::len),
        Some(1),
        "the slot filter reads one record: {by_profile:#?}"
    );

    // Revoke at a higher revision: the record keeps its member target.
    let revoke = rpc(
        &booted,
        "meta/revoke_instruction",
        json!({"namespace": "host", "key": "persona", "revision": 2}),
    )
    .await;
    let receipt = result_of(&revoke);
    assert_eq!(receipt["disposition"], json!("committed"), "{receipt:#?}");
    assert_eq!(receipt["revision"], json!(2));
    let status = rpc(&booted, "meta/instruction_status", json!({})).await;
    let status = result_of(&status);
    assert_eq!(record(status, "persona")["effect"], json!("absent"));
    assert_eq!(
        record(status, "persona")["target"],
        json!({"kind": "member", "identity": IDENTITY}),
        "{status:#?}"
    );

    // A stale revision is refused with its closed code, nothing written.
    let stale = rpc(
        &booted,
        "meta/set_instruction",
        json!({
            "namespace": "host",
            "key": "persona",
            "revision": 1,
            "body": PERSONA,
            "target": {"kind": "member", "identity": IDENTITY},
        }),
    )
    .await;
    assert_eq!(stale["error"]["code"], json!(-32602), "{stale:#?}");
    assert_eq!(
        stale["error"]["data"]["instruction_set_code"],
        json!("stale_revision"),
        "{stale:#?}"
    );

    // Unknown fields and an unknown target kind are refused before Core.
    let unknown = rpc(
        &booted,
        "meta/set_instruction",
        json!({
            "namespace": "host",
            "key": "persona",
            "revision": 3,
            "body": PERSONA,
            "target": {"kind": "member", "agent_identity": IDENTITY},
        }),
    )
    .await;
    assert_eq!(unknown["error"]["code"], json!(-32602), "{unknown:#?}");
    let after = booted
        .unified
        .mob_handle()
        .operator_instruction_status(meerkat_mob::OperatorInstructionStatusQuery::default())
        .await
        .expect("Core reads its operator instructions");
    assert_eq!(
        after.generation, 3,
        "the refused writes changed nothing: {after:#?}"
    );
    let _ = &booted.session_id;
    booted.unified.shutdown().await;
}

/// A record written for a member and its forks (Core's `include_forks`, set
/// through the mob handle a Rust host holds) reads through
/// `meta/instruction_status` with its scope, and a record for the member
/// alone reads without the member. Meta's own write verb cannot set the
/// scope: a target that carries it is refused before Core, nothing written.
#[tokio::test(flavor = "multi_thread")]
async fn meta_instruction_status_shows_a_records_fork_scope() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let booted = boot(&temp.path().join("state")).await;
    let roster_member = meerkat_meta::member_comms_id::roster_member_id_for_identity(IDENTITY);
    let write = |key: &str, include_forks: bool| meerkat_mob::OperatorInstructionWrite {
        slot: meerkat_core::InstructionSlot {
            namespace: meerkat_core::InstructionNamespace::new("host").expect("namespace"),
            key: meerkat_core::InstructionKey::new(key).expect("key"),
        },
        revision: meerkat_core::InstructionRevision::new(1).expect("revision"),
        body: PERSONA.to_string(),
        content_sha256: None,
        target: OperatorInstructionTarget::Member {
            agent_identity: roster_member.clone(),
            include_forks,
        },
    };
    for (key, include_forks) in [("family", true), ("own", false)] {
        booted
            .unified
            .mob_handle()
            .set_operator_instruction(write(key, include_forks))
            .await
            .expect("Core commits the record");
    }

    let status = rpc(&booted, "meta/instruction_status", json!({})).await;
    let status = result_of(&status);
    assert_eq!(
        record(status, "family")["target"],
        json!({"kind": "member", "identity": IDENTITY, "include_forks": true}),
        "a record for a member and its forks shows its scope: {status:#?}"
    );
    assert_eq!(
        record(status, "own")["target"],
        json!({"kind": "member", "identity": IDENTITY}),
        "a record for the member alone carries no scope member: {status:#?}"
    );

    let refused = rpc(
        &booted,
        "meta/set_instruction",
        json!({
            "namespace": "host",
            "key": "scoped",
            "revision": 1,
            "body": PERSONA,
            "target": {"kind": "member", "identity": IDENTITY, "include_forks": true},
        }),
    )
    .await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused:#?}");
    let after = booted
        .unified
        .mob_handle()
        .operator_instruction_status(meerkat_mob::OperatorInstructionStatusQuery::default())
        .await
        .expect("Core reads its operator instructions");
    assert_eq!(
        after.generation, 2,
        "the refused write changed nothing: {after:#?}"
    );
    booted.unified.shutdown().await;
}

/// The host verbs are on the gateway's host surface and in no agent tool
/// catalog: `meta/capabilities` lists all three, and the member's resolved
/// tools (builtins, comms and the mob tools turned on) carry none of them
/// and no tool that names an instruction.
#[tokio::test(flavor = "multi_thread")]
async fn meta_instruction_verbs_are_never_agent_tools() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let booted = boot(&temp.path().join("state")).await;

    let capabilities = rpc(&booted, "meta/capabilities", json!({})).await;
    let methods: Vec<&str> = result_of(&capabilities)["methods"]
        .as_array()
        .expect("method list")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for verb in INSTRUCTION_VERBS {
        assert!(
            methods.contains(&verb),
            "the gateway's host surface lists {verb}: {methods:?}"
        );
    }

    let tools = meerkat_meta::mob_handle_runtime::resolved_tools_for_session(
        booted.unified.mob_runtime().session_service(),
        IDENTITY,
        booted.session_id.clone(),
    )
    .await
    .expect("resolve the member's tools")
    .tools;
    assert!(
        tools.iter().any(|tool| tool.starts_with("mob_")),
        "the member has its agent mob tools: {tools:?}"
    );
    for tool in &tools {
        assert!(
            !tool.contains("instruction"),
            "agent tool '{tool}' reaches operator instructions, which are host verbs"
        );
        assert!(
            !INSTRUCTION_VERBS
                .iter()
                .any(|verb| verb.rsplit('/').next() == Some(tool.as_str())),
            "agent tool '{tool}' is a host verb"
        );
    }
    booted.unified.shutdown().await;
}
