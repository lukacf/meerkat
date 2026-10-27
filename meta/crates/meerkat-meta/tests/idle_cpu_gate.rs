//! Idle-CPU regression gate (2026-07).
//!
//! A fully idle meta gateway burned ~0.3 CPU cores PER durable member
//! forever: the mob actor's identity reconcile re-read and re-verified each
//! member's full persisted session document once per scan interval even when
//! nothing had changed (sha256 canonical-digest + serde of the whole
//! transcript, reached through `mob_handle_runtime` →
//! `PersistentSessionService::resolve_runtime_snapshot_read_source`), so a
//! mob of a dozen or more members could saturate a small host.
//!
//! This gate boots a real persistent gateway with multiple durable members,
//! waits for convergence, idles, and asserts the PROCESS CPU-TIME delta over
//! the idle window stays far below the historical burn. It measures CPU time
//! via `getrusage`, never wall clock. This test must stay alone in its own
//! integration binary so no sibling test's CPU pollutes the measurement.
//!
//! The threshold is deliberately generous to CI noise (10% of one core over
//! the window); the historical defect consumed ~30% of a core per member and
//! scales with member count, so a hot-loop recurrence trips this
//! immediately.
//!
//! SIZE-PROPORTIONAL class: one member carries a large (multi-megabyte)
//! persisted transcript and a console aggregator (session-history backfill
//! enabled, as every gateway runs it) is registered over the runtime. The
//! second idle-burn generation re-read that member's FULL session document
//! (two whole-document deserializes through
//! `PersistentSessionService::load_authoritative_session_base`) once per
//! 5s discovery pass and refreshed a watermark row each pass, because the
//! growing-session freshness TTL (2s) was shorter than the loop period —
//! plus the 4Hz stream reconcilers deep-cloning the machine state. With the
//! large fixture those size-proportional reads alone exceed this gate's
//! budget on pre-fix code; the committed-boundary gate and event-driven
//! reconcile cadence make them ~zero.
//!
//! COMPOSITION-PARITY class: this test composes its runtime the way the
//! production gateway binaries do (own stores + own session service +
//! `MobBootstrapSpec::new`), NOT through `UnifiedRuntimeBuilder`. The
//! third idle-burn generation survived the builder-composed version of
//! this gate: the builder path wired the console's durable-read gate
//! internally (then a write-epoch witness, now the composition's durable
//! runtime store), but the gateways' external composition left it absent,
//! silently disabling that gate in every gateway-composed runtime while the
//! test stayed green. Diverge from the gateways' composition here only with
//! a reason written down.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::identity_first::orchestrator::{RestoreOutcome, restore_flow};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, ContinuityStore, DurabilityPolicy, DurableAgentSpec,
    IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider,
    SessionBridge,
};
use meerkat_meta::mob_handle_runtime::open_native_session_pair;
use meerkat_meta::storage_provider::default_meerkat_level_realm;
use meerkat_meta::{
    AllowAllConsoleVisibilityPolicy, Base64BlobStoreAdapter, BinaryBlobStore,
    ConsoleRuntimeRegistration, DiscoverySpec, MetaConfig, MetaConsoleAggregator,
    MobBootstrapOptions, MobBootstrapSpec, ObjectStoreBlobStore, UnifiedRuntime,
};
use meerkat_mob::{MobDefinition, MobStorage, ProfileName};
use tokio::time::sleep;

const MOB_TOML: &str = r#"
[mob]
id = "idle-cpu-gate"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#;

const MEMBER_COUNT: usize = 3;
const IDLE_WINDOW: Duration = Duration::from_secs(30);
/// 10% of one core over the idle window. The pre-fix defect consumed
/// ~0.3 core-seconds per second PER member (0.9 cores for this fleet),
/// i.e. ~27 CPU-seconds over this window; the size-proportional
/// session-history re-read of the large member alone exceeds this budget.
const MAX_IDLE_CPU: Duration = Duration::from_secs(3);
/// The large member's transcript is built from turns carrying inputs of this
/// size, giving a persisted session document of
/// `LARGE_SESSION_TURNS * LARGE_TURN_INPUT_BYTES` ≈ 12 MB - a large
/// real-world scale class (synthetic; nothing committed).
const LARGE_TURN_INPUT_BYTES: usize = 3_000_000;
const LARGE_SESSION_TURNS: usize = 4;

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

fn spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity(name),
        profile: ProfileName::from("worker"),
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

/// Answers "ok" to every turn and counts requests, so turns complete without
/// a live provider.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<usize>>,
}

impl CaptureClient {
    fn count(&self) -> usize {
        *self.requests.lock().unwrap()
    }
}

impl meerkat_client::LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        *self.requests.lock().unwrap() += 1;
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            });
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

/// Total (user + system) CPU time this process has consumed since start.
fn process_cpu_time() -> Duration {
    cpu_time::ProcessTime::try_now()
        .expect("read process CPU time")
        .as_duration()
}

#[tokio::test(flavor = "multi_thread")]
async fn converged_idle_gateway_consumes_near_zero_cpu() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let capture = CaptureClient::default();

    // GATEWAY-PARITY COMPOSITION - keep in lockstep with the persistent
    // branches of src/rpc_gateway.rs / src/bin/meta_gateway.rs. Both
    // production gateways open Core's native session pair on runtime.sqlite,
    // roll their own session service and enter through
    // `MobBootstrapSpec::new`, NOT through `UnifiedRuntimeBuilder`. The 0.8.4
    // idle driver survived this gate's first version precisely because of
    // that split: the library builder declares its durable runtime store
    // internally, while the external composition must call
    // `with_durable_runtime_store` by hand. Omit it and the console
    // discovery loop re-reads whole session documents every 5s forever.
    std::fs::create_dir_all(&state_path).expect("create state dir");
    let definition = MobDefinition::from_toml(MOB_TOML).expect("parse idle-cpu mob definition");
    let (session_store, runtime_store) =
        open_native_session_pair(&state_path, &default_meerkat_level_realm())
            .expect("native session pair");
    let binary_blob_store: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(Base64BlobStoreAdapter::new(binary_blob_store));
    let adapter = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("acquire the idle CPU fixture runtime machine"),
    );
    let factory = AgentFactory::new(&state_path)
        .session_store(session_store.clone())
        .builtins(false)
        .comms(true);
    let mut agent_builder = FactoryAgentBuilder::new(factory, Config::default());
    agent_builder.default_blob_store = Some(blob_store.clone());
    let session_service: Arc<dyn meerkat_mob::MobSessionService> = Arc::new(
        PersistentSessionService::new(
            agent_builder,
            64,
            session_store,
            Arc::clone(&runtime_store),
            blob_store,
        )
        .with_canonical_runtime_adapter(adapter.clone()),
    );
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_durable_runtime_store(&runtime_store)
        .with_session_runtime_adapter(adapter)
        .expect("acquire the fixture session runtime owner")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(capture.clone())),
        });
    let unified = Arc::new(
        UnifiedRuntime::bootstrap(
            mob_spec,
            MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: "idle-cpu-gate".to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Duration::from_mins(1),
        )
        .await
        .expect("bootstrap UnifiedRuntime"),
    );
    // The library builder constructs this bridge internally after bootstrap;
    // the spec-composed (gateway-parity) path builds it by hand from the
    // same parts, exactly as `UnifiedRuntimeBuilder::build` does.
    let bridge: Arc<dyn SessionBridge> = Arc::new(
        meerkat_meta::identity_first::bridge::MobSessionBridge::with_session_service(
            unified.mob_handle(),
            unified
                .mob_runtime()
                .session_service()
                .cloned()
                .expect("spec-composed runtime carries a session service"),
        ),
    );
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db")).expect("continuity store"),
    );
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store as Arc<dyn ContinuityStore>,
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "idle-cpu-gate".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });

    let roster: Vec<DurableAgentSpec> = (0..MEMBER_COUNT)
        .map(|index| spec(&format!("worker:member-{index}")))
        .collect();
    let result = restore_flow(&identity_rt, &roster, None, None)
        .await
        .expect("restore_flow");
    for member in &roster {
        match result.outcomes.get(&member.identity) {
            Some(RestoreOutcome::Created { .. } | RestoreOutcome::Resumed { .. }) => {}
            other => panic!(
                "expected materialized member {}, got {other:?}",
                member.identity
            ),
        }
    }

    // Give every member a small persisted transcript, then let the turns and
    // their durable commits fully settle before the measured window.
    for member in &roster {
        identity_rt
            .send(
                &member.identity,
                &meerkat_core::ContentInput::Text(format!(
                    "fixture transcript for {}",
                    member.identity
                )),
            )
            .await
            .expect("seed turn");
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while capture.count() < MEMBER_COUNT {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for seed turns to reach the LLM"
        );
        sleep(Duration::from_millis(100)).await;
    }

    // Grow ONE member to a large scale (~12 MB of persisted
    // transcript, synthetic): the size-proportional idle-burn class only
    // reproduces against a large session document.
    let large_member = &roster[0].identity;
    for turn in 0..LARGE_SESSION_TURNS {
        let filler = format!("large-transcript filler {turn} ")
            .repeat(LARGE_TURN_INPUT_BYTES / 32)
            .chars()
            .take(LARGE_TURN_INPUT_BYTES)
            .collect::<String>();
        identity_rt
            .send(large_member, &meerkat_core::ContentInput::Text(filler))
            .await
            .expect("large seed turn");
    }

    // Turn-driven sends return before the session task finishes the turn and
    // its durable commits; wait until every large seed reached the LLM.
    let deadline = Instant::now() + Duration::from_mins(2);
    while capture.count() < MEMBER_COUNT + LARGE_SESSION_TURNS {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for large seed turns to reach the LLM"
        );
        sleep(Duration::from_millis(100)).await;
    }

    // Register the console aggregator exactly as the gateways do: its
    // session-history discovery loop (5s) is part of the idle surface under
    // test. In-memory store keeps the CPU class while avoiding disk noise.
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime(ConsoleRuntimeRegistration {
        runtime_key: "idle-cpu-gate".to_string(),
        runtime: Arc::clone(&unified),
        identity_namespace: "idle".to_string(),
        visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
    });

    // Quiesce before opening the measured window: the large turns' trailing
    // durable commits (multi-second, size-proportional, debug-build) and the
    // console's catch-up backfill must finish first. Wait on the console's
    // typed convergence signal: every session-history pass it owes has run,
    // including a re-read of a session whose committed boundary moved while
    // the registration pass read it, and the refresh re-driven after a
    // queued burst drains. A timed CPU probe could not see a pass scheduled
    // after it. A gateway that never converges (an idle-CPU hot loop) fails
    // here.
    tokio::time::timeout(
        Duration::from_mins(4),
        aggregator.history_backfill_converged(),
    )
    .await
    .expect("the console never converged after seeding: session-history backfill still owed");

    // The measured contract: a converged, idle gateway must consume ~zero
    // CPU regardless of member count or transcript size.
    let cpu_before = process_cpu_time();
    sleep(IDLE_WINDOW).await;
    let idle_cpu = process_cpu_time().saturating_sub(cpu_before);
    assert!(
        idle_cpu <= MAX_IDLE_CPU,
        "idle gateway burned {idle_cpu:?} CPU over {IDLE_WINDOW:?} \
         (limit {MAX_IDLE_CPU:?}); the converged fleet must be event-driven, \
         not busy re-verifying unchanged session documents"
    );

    // Cheap-idle must not mean dead: a member still serves a turn afterwards.
    let turns_before = capture.count();
    identity_rt
        .send(
            &roster[0].identity,
            &meerkat_core::ContentInput::Text("post-idle liveness probe".to_string()),
        )
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

    unified.shutdown().await;
}
