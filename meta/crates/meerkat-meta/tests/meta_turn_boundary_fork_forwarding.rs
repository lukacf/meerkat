//! Meta's session-service wrappers forward
//! `MobSessionService::fork_persisted_session_at_turn_boundary` to the
//! persistent owner as one contract.
//!
//! meerkat 0.8.41 made the method required because a default body that held
//! the persistent owner's non-reentrant boundary and then called the owner's
//! own fork self-deadlocked through every host decorator: a live delegation
//! against an idle source hung forever with no bound applied (meerkat S97,
//! 2026-09-22). `MobBootstrapSpec::new` installs Meta's decorator
//! (`PreBuildMobSessionService`) over whatever service it is given, so this
//! test runs the exact call a live delegation makes through a real
//! `UnifiedRuntime` over a real `PersistentSessionService`: a forward returns
//! `Forked` in milliseconds, a self-deadlock never returns.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService, Provider};
use meerkat_core::BlobStore;
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime,
};
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{
    ForkMemberAtTurnBoundary, MobDefinition, MobSessionService, MobStorage, SpawnMemberSpec,
};

const MOB_ID: &str = "turn-boundary-fork-forwarding";
/// The bound a live delegation applies to the boundary wait.
const FORK_BOUND: Duration = Duration::from_secs(1);
/// A self-deadlock on the owner's boundary never completes; a healthy
/// forward returns in milliseconds. Five seconds separates the two without
/// ever being close to the bound itself.
const DEADLOCK_DETECTOR: Duration = Duration::from_secs(5);

struct Harness {
    runtime: UnifiedRuntime,
    inner: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    _state: tempfile::TempDir,
}

async fn harness() -> Harness {
    let state = tempfile::tempdir().expect("state tempdir");
    let root = state.path().join("state");
    std::fs::create_dir_all(&root).expect("state directory");

    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(root.join("sessions.sqlite"))
            .expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(root.join("runtime.sqlite"))
            .expect("runtime store"),
    );
    let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
    let client = Arc::new(meerkat_client::TestClient::for_provider(Provider::OpenAI));
    let factory = AgentFactory::new(&root).comms(true).builtins(false);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    builder.default_llm_client = Some(client.clone());
    let inner = Arc::new(PersistentSessionService::new(
        builder,
        8,
        session_store,
        runtime_store,
        blob_store,
    ));

    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{MOB_ID}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("turn-boundary fork definition");
    // `MobBootstrapSpec::new` is the one layer every Meta composition
    // passes through; it wraps the given service in Meta's decorator.
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), inner.clone())
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(client),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: MOB_ID.to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("persistent UnifiedRuntime bootstrap");
    Harness {
        runtime,
        inner,
        _state: state,
    }
}

fn worker_spec(alias: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::new("worker", AgentIdentity::from(alias))
}

fn fork_target(member: &str) -> meerkat_core::DurableSessionForkTarget {
    meerkat_core::DurableSessionForkTarget {
        member_binding: meerkat_core::MobMemberBinding {
            mob_id: MOB_ID.to_string(),
            role: "worker".to_string(),
            member: member.to_string(),
        },
        cache_identity: None,
        source_admission: meerkat_core::DurableForkSourceAdmission::Quiescent,
        inherited_instructions: meerkat_core::ForkInstructionPolicy::default(),
        declared_instructions: meerkat_core::ForkInstructionPolicy::default(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn meta_wrapper_forwards_the_turn_boundary_fork_and_returns_forked_within_the_bound() {
    let harness = harness().await;
    let runtime = &harness.runtime;
    // The backing member exists and is idle: no turn is running, exactly a
    // voice executor between requests.
    let source = AgentIdentity::from("voice-executor");
    runtime
        .spawn(worker_spec("voice-executor"))
        .await
        .expect("spawn the idle source member");

    // The exact call a live delegation makes, through Meta's handle and
    // therefore through Meta's session-service decorator.
    let outcome = tokio::time::timeout(
        DEADLOCK_DETECTOR,
        runtime.mob_handle().fork_member_at_turn_boundary(
            &source,
            worker_spec("live-delegation-1"),
            None,
            FORK_BOUND,
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
    // `MobHandle` reaches it as `Arc<dyn MobSessionService>`. The runtime's
    // service is Meta's wrapper, not the persistent owner it decorates.
    let wrapper = runtime
        .mob_runtime()
        .session_service()
        .expect("a bootstrapped Meta runtime exposes its session service");
    let inner: Arc<dyn MobSessionService> = harness.inner.clone();
    assert!(
        !std::ptr::addr_eq(Arc::as_ptr(wrapper), Arc::as_ptr(&inner)),
        "the runtime must hand out Meta's decorator, not the bare persistent owner"
    );
    let source_session_id = runtime
        .mob_handle()
        .resolve_bridge_session_id(&source)
        .await
        .expect("the source member has a bridge session");
    let direct = tokio::time::timeout(
        DEADLOCK_DETECTOR,
        wrapper.fork_persisted_session_at_turn_boundary(
            &source_session_id,
            None,
            None,
            fork_target("live-delegation-2"),
            FORK_BOUND,
        ),
    )
    .await
    .expect("the decorator's turn-boundary fork must return for an idle source")
    .expect("the decorator's fork must succeed for an idle source");
    assert!(
        matches!(direct, meerkat_core::DurableForkAtTurnBoundary::Forked(_)),
        "{direct:?}"
    );

    let shutdown = runtime.shutdown().await;
    assert!(
        shutdown.cleanup_completed(),
        "shutdown must close every authority owner: {shutdown:?}"
    );
}
