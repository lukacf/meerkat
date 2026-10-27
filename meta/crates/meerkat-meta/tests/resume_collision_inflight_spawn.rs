// A roster collision against a spawn that is still in asynchronous custody is
// convergence to await, never a stale occupant to retire (production twin report
// 2026-09-22, item 3, identity review:singleton).
//
// The production sequence: a send materializes the identity and the bridge's
// resume enqueues a Spawn that sits behind a backlog (in flight, in custody).
// The client times out and re-sends, so a SECOND resume for the same member
// and session reaches the bridge while the first spawn is still building. The
// mob rejects the duplicate spawn pre-custody with `MemberAlreadyExists`; the
// bridge classified that as a stale occupant, retired (inertly: nothing was in
// the roster yet), retried the resume, collided AGAIN with the now-complete
// original spawn, and surfaced a rejection that marked the identity Broken.
// The identity never recovered.
//
// This test reproduces the race with a real mob and a real persistent session
// service: the first resume's spawn is held inside custody by an after-create
// hook that parks until a gate opens, the second resume collides while it is
// held, the gate opens 300 ms later (the original spawn completes), and BOTH
// resumes must land on the same durable session with exactly one roster
// member.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService, SessionStore};
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildDraft, AgentRuntimeServices, BridgeError, DurabilityPolicy,
    DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
    LocalLeaseProvider, MobSessionBridge, RestoreOutcome, ResumeSessionOutcome, SessionBridge,
    restore_flow,
};
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime,
};
use meerkat_mob::{MobDefinition, MobStorage};
use meerkat_runtime::SessionServiceRuntimeExt;
use tokio::sync::watch;

static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// Answers "ok" and counts requests so the seed turn can be observed.
struct AckClient {
    requests: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for AckClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let provider = LlmClient::provider(self);
        let [usage, done] =
            llm_usage::usage_then_done(request, provider, meerkat_core::StopReason::EndTurn);
        Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            }),
            Ok(usage),
            Ok(done),
        ]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct Boot {
    runtime: UnifiedRuntime,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    mob_handle: meerkat_mob::MobHandle,
    session_store: Arc<dyn SessionStore>,
    requests: Arc<AtomicUsize>,
    /// Number of member builds currently or previously parked by the gate.
    parked_builds: Arc<AtomicUsize>,
}

/// Build a mob against `state`. While `gate` is closed, every member build
/// parks inside the after-create hook: that hook runs in the mob's
/// asynchronous spawn provisioning task, so the spawn is admitted into custody
/// (pending, not yet a roster member) exactly where the production backlog
/// held spawn ticket 15.
async fn boot(state: &std::path::Path, mob_id: &str, gate: watch::Receiver<bool>) -> Boot {
    let session_store: Arc<dyn SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(state.join("sessions.db")).expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
            .expect("runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let factory = AgentFactory::new(state).comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("acquire the resume collision fixture runtime machine"),
    );
    let session_service = Arc::new(PersistentSessionService::new(
        builder,
        16,
        session_store.clone(),
        runtime_store,
        blob_store,
    ));
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.general]
model = "gpt-5.5"

[profiles.general.tools]
comms = true
"#
    ))
    .expect("definition");
    let requests = Arc::new(AtomicUsize::new(0));
    let parked_builds = Arc::new(AtomicUsize::new(0));
    let hook_parked = Arc::clone(&parked_builds);
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_session_runtime_adapter(machine.clone())
        .expect("acquire the fixture session runtime owner")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(AckClient {
                requests: Arc::clone(&requests),
            })),
        })
        .with_after_create_hook(Arc::new(move |_session_id, _context| {
            let mut gate = gate.clone();
            let parked = Arc::clone(&hook_parked);
            Box::pin(async move {
                if *gate.borrow() {
                    return;
                }
                parked.fetch_add(1, Ordering::SeqCst);
                while !*gate.borrow() {
                    if gate.changed().await.is_err() {
                        break;
                    }
                }
            })
        }));
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "inflight-collision".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap");
    let mob_handle = runtime.mob_handle();
    Boot {
        runtime,
        machine,
        mob_handle,
        session_store,
        requests,
        parked_builds,
    }
}

fn worker_spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: meerkat_meta::identity_first::AgentIdentity::parse("review:singleton")
            .expect("identity parses"),
        profile: meerkat_mob::ProfileName::from("general"),
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

fn empty_draft(spec: &DurableAgentSpec) -> AgentBuildDraft {
    AgentBuildDraft {
        compaction_curator: Default::default(),
        model: None,
        system_prompt: None,
        additional_instructions: spec.additional_instructions.clone(),
        labels: spec.labels.clone(),
        app_context: spec.context.clone(),
        external_tools: Vec::new(),
        local_external_tools: Default::default(),
        provider_params: None,
    }
}

async fn wait_until<F>(what: &str, timeout: Duration, mut condition: F)
where
    F: AsyncFnMut() -> bool,
{
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if condition().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Everything both scenarios share: boot 1 creates `review:singleton`,
/// persists one completed turn and shuts down; boot 2 comes up against the
/// same durable store with an empty roster and a store-only bridge.
struct Raced {
    _temp: tempfile::TempDir,
    boot: Boot,
    bridge: Arc<MobSessionBridge>,
    record: SeededMember,
    spec: DurableAgentSpec,
    identity: meerkat_meta::identity_first::AgentIdentity,
    gate_tx: watch::Sender<bool>,
}

/// The member boot 1 created: Core's incarnation and the session it is
/// bound to.
#[derive(Clone)]
struct SeededMember {
    agent_runtime_id: meerkat_meta::identity_first::AgentRuntimeId,
    session_id: meerkat_core::types::SessionId,
}

async fn restart_with_durable_member() -> Raced {
    let temp = tempfile::tempdir().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    let mob_id = format!(
        "inflight-collision-{}",
        NEXT_TEST_MOB_ID.fetch_add(1, Ordering::Relaxed)
    );
    let spec = worker_spec();
    let identity = spec.identity.clone();
    let (gate_tx, gate_rx) = watch::channel(true);

    // ---- Boot 1: create the identity, persist one completed turn, shut down.
    let record = {
        let boot = boot(&state, &mob_id, gate_rx.clone()).await;
        let session_service = boot
            .runtime
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("session service");
        let bridge = Arc::new(MobSessionBridge::with_session_store_and_service(
            boot.mob_handle.clone(),
            boot.session_store.clone(),
            session_service,
        ));
        let continuity_store =
            LocalContinuityStore::open(state.join("continuity.db")).expect("continuity store");
        let irt = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(continuity_store),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "inflight-collision-boot-1".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge.clone()),
                default_timeout: None,
            })
            .with_runtime_services(AgentRuntimeServices::new(boot.mob_handle.clone())),
        );
        let result = restore_flow(&irt, std::slice::from_ref(&spec), None, None)
            .await
            .expect("restore_flow");
        let record = match result.outcomes.get(&identity) {
            Some(RestoreOutcome::Created { binding, .. }) => SeededMember {
                agent_runtime_id: binding.agent_runtime_id.clone(),
                session_id: binding
                    .session_id
                    .clone()
                    .expect("Core binds a fresh member to a session"),
            },
            other => panic!("boot 1 must fresh-create, got {other:?}"),
        };
        bridge
            .deliver(
                &record.agent_runtime_id,
                &meerkat_core::ContentInput::Text("seed turn".to_string()),
            )
            .await
            .expect("seed turn delivery");
        let machine = boot.machine.clone();
        let requests = boot.requests.clone();
        let session_id = record.session_id.clone();
        wait_until("the seed turn to complete", Duration::from_secs(20), || {
            let machine = machine.clone();
            let requests = requests.clone();
            let session_id = session_id.clone();
            async move {
                requests.load(Ordering::SeqCst) >= 1
                    && SessionServiceRuntimeExt::list_active_inputs(machine.as_ref(), &session_id)
                        .await
                        .map(|active| active.is_empty())
                        .unwrap_or(false)
            }
        })
        .await;
        // Let the assistant response commit before the shutdown flush.
        tokio::time::sleep(Duration::from_millis(500)).await;
        boot.runtime.shutdown().await;
        record
    };

    // ---- Boot 2: same durable store, empty roster.
    let boot = boot(&state, &mob_id, gate_rx).await;
    let bridge = Arc::new(
        MobSessionBridge::with_session_store(boot.mob_handle.clone(), boot.session_store.clone())
            .with_actor_admission_budget(Duration::from_secs(20)),
    );
    assert!(
        boot.mob_handle.list_all_members().await.is_empty(),
        "boot 2 must start with an empty roster"
    );
    Raced {
        _temp: temp,
        boot,
        bridge,
        record,
        spec,
        identity,
        gate_tx,
    }
}

fn spawn_resume(
    raced: &Raced,
) -> tokio::task::JoinHandle<Result<ResumeSessionOutcome, BridgeError>> {
    let bridge = raced.bridge.clone();
    let identity = raced.identity.clone();
    let spec = raced.spec.clone();
    let record = raced.record.clone();
    tokio::spawn(async move {
        bridge
            .resume_session(
                &identity,
                &spec,
                &empty_draft(&spec),
                &record.session_id,
                None,
            )
            .await
    })
}

/// The post-race invariant both scenarios share: one roster member, bound to
/// the durable session.
async fn assert_single_member_bound(raced: &Raced) {
    let members = raced.boot.mob_handle.list_all_members().await;
    assert_eq!(
        members.len(),
        1,
        "exactly one roster member must exist after the race, got {members:?}"
    );
    let member_session = raced
        .boot
        .mob_handle
        .resolve_bridge_session_id(&members[0].agent_identity)
        .await;
    assert_eq!(
        member_session.as_ref(),
        Some(&raced.record.session_id),
        "the single member must be bound to the durable session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_collision_with_in_flight_spawn_awaits_convergence_and_attaches() {
    let raced = restart_with_durable_member().await;
    let record = raced.record.clone();

    // Close the gate: the first resume's spawn is admitted into custody and
    // parks inside its member build.
    raced.gate_tx.send(false).expect("close gate");
    let first = spawn_resume(&raced);
    let parked = raced.boot.parked_builds.clone();
    wait_until(
        "the first resume's spawn to park inside custody",
        Duration::from_secs(20),
        || {
            let parked = parked.clone();
            async move { parked.load(Ordering::SeqCst) >= 1 }
        },
    )
    .await;
    assert!(
        raced.boot.mob_handle.list_all_members().await.is_empty(),
        "a spawn in custody must not yet be a roster member"
    );

    // The client retry: a second resume for the same member and session while
    // the first spawn is still in custody. It collides pre-custody.
    let second = spawn_resume(&raced);
    // Production timing: the original spawn completed ~250 ms after the collision.
    tokio::time::sleep(Duration::from_millis(300)).await;
    raced.gate_tx.send(true).expect("open gate");

    let first = tokio::time::timeout(Duration::from_mins(1), first)
        .await
        .expect("first resume must settle")
        .expect("first resume task must not panic");
    let second = tokio::time::timeout(Duration::from_mins(1), second)
        .await
        .expect("second resume must settle")
        .expect("second resume task must not panic");

    let first = first.expect("the original resume must succeed");
    assert_eq!(
        first.session_id(),
        &record.session_id,
        "the original resume must land on the durable session"
    );
    let second = second.expect(
        "a resume that collides with an in-flight spawn for the same member and session must \
         await that spawn and attach, never retire it and fail",
    );
    assert_eq!(
        second.session_id(),
        &record.session_id,
        "the colliding resume must attach to the same durable session"
    );
    assert!(
        first.fallback_reason().is_none() && second.fallback_reason().is_none(),
        "neither resume may degrade to a fresh spawn: {first:?} / {second:?}"
    );
    assert!(
        matches!(
            second,
            ResumeSessionOutcome::Resumed { .. }
                | ResumeSessionOutcome::AttachedPendingRegistration { .. }
        ),
        "the colliding resume must report an attach, got {second:?}"
    );
    assert_single_member_bound(&raced).await;

    raced.boot.runtime.shutdown().await;
}

/// The other side of the production window: the spawn in custody completes BEFORE the
/// colliding resume classifies the occupant. The collision then names a
/// committed roster member that is already bound to the very session being
/// resumed. That member is healthy and it IS the resume target, so it must be
/// adopted, not retired and re-resumed: the same roster row (runtime id,
/// generation, fence) must survive the second resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_collision_with_a_completed_spawn_adopts_the_bound_member() {
    let raced = restart_with_durable_member().await;
    let record = raced.record.clone();

    // First resume materializes the member completely (gate open).
    let first = tokio::time::timeout(Duration::from_mins(1), spawn_resume(&raced))
        .await
        .expect("first resume must settle")
        .expect("first resume task must not panic")
        .expect("the original resume must succeed");
    assert_eq!(first.session_id(), &record.session_id);
    assert_single_member_bound(&raced).await;
    let before = raced.boot.mob_handle.list_all_members().await;

    // The client retry arrives after the spawn finished: a roster collision
    // against a committed member bound to the resumed session.
    let second = tokio::time::timeout(Duration::from_mins(1), spawn_resume(&raced))
        .await
        .expect("second resume must settle")
        .expect("second resume task must not panic")
        .expect(
            "a resume that collides with a committed member already bound to the resumed \
             session must adopt it, never retire it",
        );
    assert_eq!(
        second.session_id(),
        &record.session_id,
        "the colliding resume must attach to the same durable session"
    );
    assert!(
        second.needs_owner_registration(),
        "a direct bridge adoption must report the attach as pending owner registration, \
         got {second:?}"
    );

    // No retire ran: the very same roster incarnation is still the member.
    assert_single_member_bound(&raced).await;
    let after = raced.boot.mob_handle.list_all_members().await;
    assert_eq!(
        (
            &after[0].agent_runtime_id,
            after[0].generation,
            after[0].fence_token
        ),
        (
            &before[0].agent_runtime_id,
            before[0].generation,
            before[0].fence_token
        ),
        "adoption must leave the committed member's incarnation untouched (a retire and \
         re-resume would mint a new one)"
    );

    raced.boot.runtime.shutdown().await;
}
