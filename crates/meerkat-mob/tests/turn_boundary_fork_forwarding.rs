//! A live delegation forks its source member at the source's turn boundary
//! through `MobSessionService::fork_persisted_session_at_turn_boundary`.
//! Hosts (the RPC server, the CLI) reach the persistent session owner
//! through a decorator that forwards trait methods one by one. The method is
//! required so a decorator cannot compile without forwarding it: a default
//! body that held the persistent owner's non-reentrant boundary across the
//! owner's own fork made a delegation against an idle source hang forever
//! with no bound applied (S97, 2026-09-22). This test keeps the wrapper shape
//! honest end to end: a real persistent service behind a forwarding
//! decorator, an idle member, the exact call a live delegation makes.

#![cfg(all(not(target_arch = "wasm32"), feature = "test-support"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_mob::definition::WiringRules;
use meerkat_mob::{
    AgentIdentity, ForkMemberAtTurnBoundary, MobBuilder, MobDefinition, MobId, MobRuntimeMode,
    MobSessionService, MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec,
    ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, StoreAdapter};

struct Paths {
    user_config_root: PathBuf,
    runtime_root: PathBuf,
    project_root: PathBuf,
    context_root: PathBuf,
    sessions_root: PathBuf,
    mob_db_path: PathBuf,
}

impl Paths {
    fn new(root: &Path) -> Self {
        Self {
            user_config_root: root.join("user-config"),
            runtime_root: root.join("runtime-root"),
            project_root: root.join("project-root"),
            context_root: root.join("context-root"),
            sessions_root: root.join("sessions-jsonl"),
            mob_db_path: root.join("mob.db"),
        }
    }
}

fn persistent_service(paths: &Paths) -> Arc<PersistentSessionService<FactoryAgentBuilder>> {
    for root in [&paths.project_root, &paths.context_root] {
        std::fs::create_dir_all(root).expect("create project/context root");
        std::fs::write(root.join("AGENTS.md"), "# Turn boundary fork\n").expect("write AGENTS.md");
    }
    let factory = AgentFactory::new(paths.runtime_root.join("factory-store"))
        .user_config_root(paths.user_config_root.clone())
        .runtime_root(paths.runtime_root.clone())
        .project_root(paths.project_root.clone())
        .context_root(paths.context_root.clone())
        .builtins(true)
        .comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
        meerkat_core::Provider::OpenAI,
    )));
    let store = Arc::new(JsonlStore::new(paths.sessions_root.clone()));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));
    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::default());
    Arc::new(PersistentSessionService::new(
        builder,
        32,
        store_dyn,
        runtime_store,
        blob_store,
    ))
}

fn fork_target(member: &str) -> meerkat_core::DurableSessionForkTarget {
    meerkat_core::DurableSessionForkTarget {
        member_binding: meerkat_core::MobMemberBinding {
            mob_id: "voice".to_string(),
            role: "executor".to_string(),
            member: member.to_string(),
        },
        cache_identity: None,
        source_admission: meerkat_core::DurableForkSourceAdmission::Quiescent,
    }
}

fn definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("executor"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: "gpt-5.4".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "Executes voice requests".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "voice-fork-{}",
        meerkat_core::time_compat::new_uuid_v7()
    )));
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![],
    };
    definition
}

#[tokio::test]
async fn turn_boundary_fork_through_a_forwarding_host_returns_within_the_bound() {
    let root = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(root.path());
    let inner = persistent_service(&paths);
    // The host decorator shape (see `FailingOnceSessionService`): fork, turn
    // boundary, and turn-boundary fork forwarded to the persistent owner.
    let host = support::FailingOnceSessionService::new(
        Arc::clone(&inner) as Arc<dyn MobSessionService>,
        None,
        None,
        0,
        false,
    );
    let storage = MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage");
    let handle = MobBuilder::new(definition(), storage)
        .with_session_service(Arc::clone(&host) as Arc<dyn MobSessionService>)
        .with_default_llm_client(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )))
        .create()
        .await
        .expect("create mob");
    // The backing member exists and is idle: no turn is running, exactly a
    // voice executor between requests.
    let source = AgentIdentity::from("voice-executor");
    handle
        .spawn_spec(SpawnMemberSpec::new("executor", source.clone()))
        .await
        .expect("spawn the executor");

    // The exact call a live delegation makes, with a 1 s bound under a 5 s
    // detector: a self-deadlock on the owner's boundary never completes, a
    // healthy handover returns in milliseconds.
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        handle.fork_member_at_turn_boundary(
            &source,
            SpawnMemberSpec::new("executor", AgentIdentity::from("live-delegation-1")),
            None,
            Duration::from_secs(1),
        ),
    )
    .await
    .expect("the turn-boundary fork must return for an idle source, not hang on its own boundary")
    .expect("the fork must succeed for an idle source");
    let ForkMemberAtTurnBoundary::Forked(forked) = outcome else {
        panic!("an idle source is not busy: {outcome:?}");
    };
    assert_eq!(
        forked.agent_identity,
        AgentIdentity::from("live-delegation-1")
    );

    // The same through the decorator's trait method directly, the way
    // `MobHandle` reaches it as `Arc<dyn MobSessionService>`.
    let source_session_id = handle
        .resolve_bridge_session_id(&source)
        .await
        .expect("source bridge session");
    let direct = tokio::time::timeout(
        Duration::from_secs(5),
        host.fork_persisted_session_at_turn_boundary(
            &source_session_id,
            None,
            None,
            fork_target("live-delegation-2"),
            Duration::from_secs(1),
        ),
    )
    .await
    .expect("the decorator's turn-boundary fork must return for an idle source")
    .expect("the decorator's fork must succeed for an idle source");
    assert!(
        matches!(direct, meerkat_core::DurableForkAtTurnBoundary::Forked(_)),
        "{direct:?}"
    );
}
