#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! Idle-CPU regression gate for the mob runtime (turbo-s smoke lane).
//!
//! Three releases in a row shipped fixed-cadence idle loops that did
//! O(fleet-state) or O(session-document) work per tick (25ms identity
//! reconcile with full-document sha256 verification, ~1s scan verify, 250ms
//! monitor loops deep-cloning `MobMachineState`). Every suite passed because
//! fixtures were KB-scale; an 82MB production session dump was the only thing
//! that caught them. This gate boots a persistent 3-member mob with one
//! production-scale (~12MB) session document, waits for convergence, idles,
//! and asserts the PROCESS CPU-TIME delta over the idle window stays far
//! below the historical burn. It measures the driver (any fixed-cadence idle
//! work), so it asserts CPU time via `cpu_time::ProcessTime`, never wall
//! clock or %CPU sampling. This test must stay ALONE in its test binary so no
//! sibling test's CPU pollutes the measurement.
//!
//! No live provider is involved: members run against a scripted LLM client,
//! so the lane needs no API keys and the measured window is deterministic.
//!
//! Run with:
//!   cargo test -p meerkat-mob --test smoke_mob_idle_burn \
//!     --features integration-real-tests -- --ignored --nocapture

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_core::types::HandlingMode;
use meerkat_mob::definition::{OrchestratorConfig, WiringRules};
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus, MobRuntimeMode,
    MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, MemoryBlobStore, StoreAdapter};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::time::{Duration, Instant, sleep};

/// 10% of one core over the idle window. The historical defects consumed
/// ~30% of a core (and scaled with member count / document size), i.e.
/// ~9+ CPU-seconds over this window; a hot-loop recurrence trips this
/// immediately while CI noise stays well under it.
const IDLE_WINDOW: Duration = Duration::from_secs(30);
const MAX_IDLE_CPU: Duration = Duration::from_secs(3);

/// The large member's transcript is grown from turns carrying inputs of this
/// size, giving a persisted session document of at least
/// `LARGE_SESSION_TURNS * LARGE_TURN_INPUT_BYTES` ≈ 12 MB — the
/// production-dump scale class where size-proportional idle reads become
/// visible (synthetic; nothing committed).
const LARGE_TURN_INPUT_BYTES: usize = 3_000_000;
const LARGE_SESSION_TURNS: usize = 4;
const MIN_PERSISTED_BYTES: u64 = 10 * 1024 * 1024;

const MEMBER_IDS: [&str; 3] = ["lead-1", "w-1", "w-2"];
const LARGE_MEMBER_ID: &str = "w-1";

/// Answers "ok" to every turn and counts requests, so member turns complete
/// deterministically without a live provider and the test can observe turn
/// completion.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<AtomicUsize>,
}

impl CaptureClient {
    fn count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let events = vec![
            meerkat_client::LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            },
            meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            },
        ];
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// Total (user + system) CPU time this process has consumed since start.
fn process_cpu_time() -> Duration {
    cpu_time::ProcessTime::try_now()
        .expect("read process CPU time")
        .as_duration()
}

fn idle_profile(peer_description: &str) -> Profile {
    Profile {
        model: "gpt-5.5".to_string(),
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
        peer_description: peer_description.to_string(),
        external_addressable: true,
        backend: None,
        runtime_mode: MobRuntimeMode::TurnDriven,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
    }
}

fn idle_mob_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        ProfileBinding::Inline(Box::new(idle_profile("Leads the idle-burn gate mob"))),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(idle_profile("Idle-burn gate worker"))),
    );

    let mut definition = MobDefinition::explicit(MobId::from("idle-burn-gate"));
    definition.orchestrator = Some(OrchestratorConfig {
        profile: ProfileName::from("lead"),
    });
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: true,
        role_wiring: vec![],
    };
    definition
}

/// Recursive on-disk size of the persisted session root. The historical
/// regressions were store-read/digest loops, so the gate verifies the large
/// document is actually durable (not just live in memory) before idling.
fn dir_size_bytes(root: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => dir_size_bytes(&path),
                Ok(meta) => meta.len(),
                Err(_) => 0,
            }
        })
        .sum()
}

async fn wait_for_requests(capture: &CaptureClient, at_least: usize, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while capture.count() < at_least {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {} of {at_least} LLM requests observed",
            capture.count()
        );
        sleep(Duration::from_millis(100)).await;
    }
}

async fn active_member_count(handle: &MobHandle) -> usize {
    handle
        .list_members()
        .await
        .into_iter()
        .filter(|entry| entry.status == MobMemberStatus::Active)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_mob_idle_burn_gate() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    let user_config_root = root.join("user-config");
    let runtime_root = root.join("runtime-root");
    let project_root = root.join("project-root");
    let context_root = root.join("context-root");
    let sessions_root = root.join("sessions-jsonl");
    let mob_db_path = root.join("mob.db");
    for dir in [&project_root, &context_root] {
        fs::create_dir_all(dir).expect("create idle-burn project/context root");
    }

    let capture = CaptureClient::default();

    let factory = AgentFactory::new(runtime_root.join("factory-store"))
        .user_config_root(user_config_root)
        .runtime_root(runtime_root)
        .project_root(project_root)
        .context_root(context_root)
        .builtins(true)
        .comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    let store = Arc::new(JsonlStore::new(sessions_root.clone()));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::default());
    let service = Arc::new(PersistentSessionService::new(
        builder,
        32,
        store_dyn,
        runtime_store,
        blob_store,
    ));

    let boot_start = Instant::now();
    let storage = MobStorage::persistent(&mob_db_path).expect("create persistent mob storage");
    let handle = MobBuilder::new(idle_mob_definition(), storage)
        .with_session_service(service.clone())
        .with_default_llm_client(Arc::new(capture.clone()))
        .create()
        .await
        .expect("create persistent idle-burn mob");

    handle
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("w-1")))
        .await
        .expect("spawn worker 1");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("w-2")))
        .await
        .expect("spawn worker 2");

    let deadline = Instant::now() + Duration::from_secs(60);
    while active_member_count(&handle).await < MEMBER_IDS.len() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {} active members; roster: {:?}",
            MEMBER_IDS.len(),
            handle.list_members().await
        );
        sleep(Duration::from_millis(100)).await;
    }
    let boot_to_ready = boot_start.elapsed();
    eprintln!(
        "[idle-burn gate] boot-to-ready (build start → {MEMBER_IDS:?} active): {boot_to_ready:?}"
    );

    // Give every member a small persisted transcript so all three sessions
    // participate in whatever the idle path scans.
    for member_id in MEMBER_IDS {
        handle
            .member(&AgentIdentity::from(member_id))
            .await
            .expect("member handle")
            .send(
                format!("fixture transcript for {member_id}"),
                HandlingMode::Queue,
            )
            .await
            .expect("seed turn");
    }
    wait_for_requests(&capture, MEMBER_IDS.len(), "seed turns").await;

    // Grow ONE member to production-dump scale (~12 MB of persisted
    // transcript): the size-proportional idle-burn class only reproduces
    // against a large session document.
    let large_member = handle
        .member(&AgentIdentity::from(LARGE_MEMBER_ID))
        .await
        .expect("large member handle");
    for turn in 0..LARGE_SESSION_TURNS {
        let filler = format!("large-transcript filler {turn} ")
            .repeat(LARGE_TURN_INPUT_BYTES / 32)
            .chars()
            .take(LARGE_TURN_INPUT_BYTES)
            .collect::<String>();
        large_member
            .send(filler, HandlingMode::Queue)
            .await
            .expect("large seed turn");
    }
    wait_for_requests(
        &capture,
        MEMBER_IDS.len() + LARGE_SESSION_TURNS,
        "large seed turns",
    )
    .await;

    // The regression class under test is store-read/digest loops, so the
    // large document must actually be durable in the session store before
    // the measured window opens.
    let deadline = Instant::now() + Duration::from_secs(120);
    let persisted_bytes = loop {
        let bytes = dir_size_bytes(&sessions_root);
        if bytes >= MIN_PERSISTED_BYTES {
            break bytes;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the large session document to persist: \
             {bytes} bytes on disk (need >= {MIN_PERSISTED_BYTES})"
        );
        sleep(Duration::from_millis(250)).await;
    };
    eprintln!(
        "[idle-burn gate] persisted session store size: {:.1} MB",
        persisted_bytes as f64 / (1024.0 * 1024.0)
    );

    // Quiesce before opening the measured window: trailing durable commits of
    // the large turns are legitimate work. Probe the process CPU rate until a
    // 2s probe reads idle-level; a mob that NEVER quiesces fails here — which
    // is exactly the defect class this gate exists to catch.
    let quiesce_deadline = Instant::now() + Duration::from_secs(240);
    loop {
        let probe_start = process_cpu_time();
        sleep(Duration::from_secs(2)).await;
        let probe_burn = process_cpu_time().saturating_sub(probe_start);
        if probe_burn < Duration::from_millis(200) {
            break;
        }
        assert!(
            Instant::now() < quiesce_deadline,
            "mob never quiesced after seeding: still burning {probe_burn:?} \
             per 2s probe (an idle-CPU hot loop)"
        );
    }

    // The measured contract: a converged, idle mob must consume ~zero CPU
    // regardless of member count or session-document size.
    let cpu_before = process_cpu_time();
    sleep(IDLE_WINDOW).await;
    let idle_cpu = process_cpu_time().saturating_sub(cpu_before);
    eprintln!("[idle-burn gate] idle CPU over {IDLE_WINDOW:?}: {idle_cpu:?}");
    assert!(
        idle_cpu <= MAX_IDLE_CPU,
        "idle mob burned {idle_cpu:?} CPU over {IDLE_WINDOW:?} (limit {MAX_IDLE_CPU:?}); \
         the converged fleet must be event-driven, not busy re-reading or \
         re-verifying unchanged session documents"
    );

    // Cheap idle must not mean dead: the roster is still observable and a
    // member still serves a turn afterwards.
    assert_eq!(
        handle.list_members().await.len(),
        MEMBER_IDS.len(),
        "post-idle roster lost members"
    );
    let turns_before = capture.count();
    handle
        .member(&AgentIdentity::from("lead-1"))
        .await
        .expect("post-idle member handle")
        .send("post-idle liveness probe".to_string(), HandlingMode::Queue)
        .await
        .expect("post-idle turn");
    let deadline = Instant::now() + Duration::from_secs(30);
    while capture.count() <= turns_before {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the post-idle turn to reach the LLM"
        );
        sleep(Duration::from_millis(100)).await;
    }

    handle.shutdown().await.expect("shutdown idle-burn mob");
}
