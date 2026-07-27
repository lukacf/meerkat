#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! Turn-latency size-independence gate for the mob runtime (turbo-s smoke
//! lane), sibling to `smoke_mob_idle_burn`.
//!
//! The production defect this pins (measured 2026-07-25 on a live 0.8.6
//! fleet): identical one-word-ACK turns took 60 seconds at a 14 MB session
//! and over 180 seconds at 94 MB, because turn-boundary work — canonical
//! serialization + SHA-256 over the whole session document, authority
//! reloads that re-decode the full snapshot, and whole-blob persistence —
//! is O(document) regardless of how small the turn's actual delta is.
//!
//! The asserted contract is SIZE INDEPENDENCE, not "faster than before":
//! a threshold calibrated against today's cost rots, and "2x faster" still
//! scales. Two members are grown to very different transcript sizes in the
//! SAME process on the SAME machine, both are driven through N identical
//! tiny turns, and the per-turn cost of the large member must stay within a
//! small constant factor of the small member's. Any O(document) pass left
//! on the turn boundary makes the large member's per-turn cost track its
//! document size and trips the ratio.
//!
//! The ASSERTED signal is canonical bytes hashed per turn
//! (`meerkat_core::global_session_content_digest_bytes`, a process-wide
//! atomic, so tokio workers and `spawn_blocking` threads are all counted):
//! deterministic, content-driven, and immune to the false-green modes a
//! time ratio permits — scheduler contention inflating the small side
//! "improves" a wall/CPU ratio with zero real gain (observed live:
//! 58.1x -> 17.2x from contention alone). Bytes are asserted BOTH
//! absolutely (a small-side band pins the denominator against inflation)
//! and relatively (the large side must stay within a small factor of the
//! small side). Process CPU time and wall time per turn — the thing users
//! actually feel — are printed as diagnostics but never asserted. The
//! per-thread `session_content_digest_computations` counter is NOT used:
//! it is `thread_local!` and under-counts from the test thread under the
//! multi-threaded runtime.
//!
//! This test must stay ALONE in its test binary so no sibling test's CPU
//! pollutes the measurement. No live provider is involved: members run
//! against a scripted LLM client, so the lane needs no API keys.
//!
//! Run with:
//!   cargo test -p meerkat-mob --test smoke_mob_turn_latency \
//!     --features integration-real-tests -- --ignored --nocapture

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_core::types::HandlingMode;
use meerkat_mob::definition::{OrchestratorConfig, WiringRules};
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus, MobRuntimeMode,
    MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, SqliteSessionStore, StoreAdapter};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::time::{Duration, Instant, sleep};

/// One-word ACK driven at both fixtures. Identical inputs are the point:
/// only the accumulated document size differs between the two measurements.
const MEASURED_TURN_PROMPT: &str = "ack?";
const MEASURED_TURNS: usize = 4;

/// The small member's transcript: one modest seed turn (~256 KB), so the
/// baseline exercises the same boundary machinery over a real but small
/// document.
const SMALL_SEED_INPUT_BYTES: usize = 256 * 1024;

/// The large member's transcript is grown from turns carrying inputs of
/// this size, giving an accumulated transcript of at least
/// `LARGE_SESSION_TURNS * LARGE_TURN_INPUT_BYTES` ≈ 10 MB — the
/// production-scale class where O(document) turn-boundary work became
/// minutes per turn (synthetic; nothing committed).
const LARGE_TURN_INPUT_BYTES: usize = 2_500_000;
const LARGE_SESSION_TURNS: usize = 4;

/// The durable stores must grow by at least this much after the large
/// member is seeded, proving the large fixture is actually large ON DISK
/// (a green gate with a small-on-disk "large" fixture would be measuring
/// nothing).
const MIN_LARGE_GROWTH_BYTES: u64 = 8 * 1024 * 1024;

/// Flatness tolerance: canonical bytes hashed per turn at ~10 MB must stay
/// within K× the bytes per turn at ~256 KB, plus a fixed allowance. With
/// turn cost truly independent of document size the ratio is ~1 (same fixed
/// path, delta-only content), so K = 3 leaves headroom for legitimate
/// O(delta) variance — while any O(document) boundary pass at a ~40× size
/// ratio measures far above it. The fixed allowance exists because the
/// flat-curve work drove the small-side denominator to ~0 hashed bytes per
/// ordinary turn (recalibrated 2026-07-27, was ~6 MB/turn at calibration
/// 2026-07-26): a pure ratio over a zero denominator rejects even one stray
/// kilobyte, while the defect class this gate pins is whole-document passes
/// — the allowance is far below one ~9 MB document pass, so a single
/// O(document) regression still trips it. Bytes are deterministic, so
/// unlike the retired CPU-ratio form this needs no scheduler-noise
/// allowance.
const MAX_LARGE_TO_SMALL_BYTES_RATIO: u64 = 3;
const LARGE_DIGEST_FIXED_ALLOWANCE_BYTES: u64 = 4 * 1024 * 1024;

/// Sanity bounds that keep the flatness assertion honest (recalibrated
/// 2026-07-27 for the flat-curve work; the former 1 MiB digest FLOOR is
/// deliberately retired — ordinary turns now hash ~0 canonical bytes, which
/// is the contract, not a broken instrument).
///
/// Instrument honesty moves to the ENCODE counter: the boundary contract
/// still serializes the whole document once per boundary commit, so a
/// fixture whose measured turns produced fewer serialized bytes than one
/// small document per turn is not driving real boundary commits (hollow
/// fixture / dead counters), and no digest conclusion drawn from it means
/// anything. The digest CEILING still catches a small-side baseline that
/// regressed or was inflated to launder the ratio.
const SMALL_DIGEST_BYTES_PER_TURN_MAX: u64 = 4 * 1024 * 1024;
const SMALL_ENCODE_BYTES_PER_TURN_MIN: u64 = 64 * 1024;

const MEMBER_IDS: [&str; 3] = ["lead-1", "w-small", "w-large"];
const SMALL_MEMBER_ID: &str = "w-small";
const LARGE_MEMBER_ID: &str = "w-large";

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

fn gate_profile(peer_description: &str) -> Profile {
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

fn gate_mob_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        ProfileBinding::Inline(Box::new(gate_profile("Leads the turn-latency gate mob"))),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(gate_profile("Turn-latency gate worker"))),
    );

    let mut definition = MobDefinition::explicit(MobId::from("turn-latency-gate"));
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

/// Recursive on-disk size of the durable store root (SQLite dbs + WAL
/// sidecars). Used to prove the large fixture is actually large on disk
/// before anything is measured.
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
    let deadline = Instant::now() + Duration::from_secs(240);
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

/// Wait until a 2s process-CPU probe reads idle-level, so trailing durable
/// boundary commits are finished (and their CPU is attributed) before a
/// measurement window closes or the next one opens.
async fn quiesce(what: &str) {
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        let probe_start = process_cpu_time();
        sleep(Duration::from_secs(2)).await;
        let probe_burn = process_cpu_time().saturating_sub(probe_start);
        if probe_burn < Duration::from_millis(200) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "mob never quiesced {what}: still burning {probe_burn:?} per 2s probe"
        );
    }
}

struct TurnCost {
    cpu_per_turn: Duration,
    wall_per_turn: Duration,
    /// Canonical bytes hashed per turn (process-wide counter delta). The
    /// asserted signal; CPU and wall are diagnostics.
    digest_bytes_per_turn: u64,
    /// Whole-session boundary-serialization output bytes per turn
    /// (process-wide counter delta). A digest-flat turn can still hide an
    /// O(document) reserialize that hashes nothing; this counter sees it.
    encode_bytes_per_turn: u64,
}

/// Drive `MEASURED_TURNS` identical tiny turns at one member and return the
/// per-turn process-CPU and wall cost. The window opens after a quiesce and
/// closes after the trailing quiesce, so asynchronous turn-boundary
/// persistence is attributed to the turn that caused it.
async fn measure_member_turns(
    handle: &MobHandle,
    capture: &CaptureClient,
    member_id: &str,
) -> TurnCost {
    quiesce(&format!("before measuring {member_id}")).await;
    let member = handle
        .member(&AgentIdentity::from(member_id))
        .await
        .expect("measured member handle");

    let cpu_start = process_cpu_time();
    let digest_bytes_start = meerkat_core::global_session_content_digest_bytes();
    let encode_bytes_start = meerkat_core::global_session_encode_bytes();
    let digest_sites_start = meerkat_core::digest_site_bytes();
    let wall_start = Instant::now();
    for turn in 0..MEASURED_TURNS {
        let expected = capture.count() + 1;
        member
            .send(MEASURED_TURN_PROMPT.to_string(), HandlingMode::Queue)
            .await
            .expect("measured turn send");
        wait_for_requests(
            capture,
            expected,
            &format!("measured turn {turn} at {member_id}"),
        )
        .await;
    }
    quiesce(&format!("after measuring {member_id}")).await;
    let cpu = process_cpu_time().saturating_sub(cpu_start);
    let wall = wall_start.elapsed();
    let digest_bytes = meerkat_core::global_session_content_digest_bytes()
        .saturating_sub(digest_bytes_start)
        / MEASURED_TURNS as u64;
    let encode_bytes = meerkat_core::global_session_encode_bytes()
        .saturating_sub(encode_bytes_start)
        / MEASURED_TURNS as u64;
    eprintln!(
        "[turn-latency gate] {member_id}: {} MB canonicalized-and-hashed per turn",
        digest_bytes / (1024 * 1024)
    );
    let digest_sites_end = meerkat_core::digest_site_bytes();
    for (index, label) in meerkat_core::DIGEST_SITE_LABELS.iter().enumerate() {
        let site_bytes = digest_sites_end[index].saturating_sub(digest_sites_start[index])
            / MEASURED_TURNS as u64;
        eprintln!(
            "[turn-latency gate]   {member_id} site {label}: {} MB per turn",
            site_bytes / (1024 * 1024)
        );
    }

    eprintln!(
        "[turn-latency gate] {member_id}: {} MB boundary-serialized per turn",
        encode_bytes / (1024 * 1024)
    );

    TurnCost {
        cpu_per_turn: cpu / MEASURED_TURNS as u32,
        wall_per_turn: wall / MEASURED_TURNS as u32,
        digest_bytes_per_turn: digest_bytes,
        encode_bytes_per_turn: encode_bytes,
    }
}

/// Shared harness: boots the 3-member mob, grows the fixtures, measures
/// both members, asserts fixture validity and the small-side band, and
/// returns the two measurements for the caller's own assertions.
async fn run_turn_latency_harness() -> (TurnCost, TurnCost) {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    let user_config_root = root.join("user-config");
    let runtime_root = root.join("runtime-root");
    let project_root = root.join("project-root");
    let context_root = root.join("context-root");
    let stores_root = root.join("stores");
    let mob_db_path = root.join("mob.db");
    for dir in [&project_root, &context_root, &stores_root] {
        fs::create_dir_all(dir).expect("create turn-gate roots");
    }

    let capture = CaptureClient::default();

    let factory = AgentFactory::new(runtime_root.join("factory-store"))
        .user_config_root(user_config_root)
        .runtime_root(runtime_root)
        .project_root(project_root)
        .context_root(context_root)
        .builtins(true)
        .comms(true);
    let mut config = Config::default();
    // Fixture comparability (deliberate, 2026-07-27): at the default
    // 100k-token threshold the ~10 MB member auto-compacts on EVERY measured
    // turn while the ~256 KB member never does, so the two windows measure
    // different operations. A compaction turn is inherently O(document) at
    // least once — the rebuilt transcript must be hashed and persisted — so
    // "flat compaction" is not a coherent contract; the flatness contract is
    // the ORDINARY turn (exactly the production defect shape: the live
    // 60 s/180 s one-word turns were not compacting). Raising the threshold
    // far above both fixtures makes both windows measure the identical
    // append-boundary operation. The compaction path's own ~6x redundancy
    // multiplier is pinned separately by the meerkat-core digest-budget and
    // rewrite-commit tests.
    config.compaction.auto_compact_threshold = 50_000_000;
    let mut builder = FactoryAgentBuilder::new(factory, config);
    // Production shape: an INCREMENTAL SQLite session store (so boundary
    // saves take the O(delta)-rows projection branch — the path the fix
    // must make flat) and a SQLite runtime store (whole-blob snapshot
    // commits with their decode + save-guard costs).
    let store =
        Arc::new(SqliteSessionStore::open(stores_root.join("sessions.db")).expect("session store"));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new(stores_root.join("runtime.db"))
            .expect("runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::default());
    let service = Arc::new(PersistentSessionService::new(
        builder,
        32,
        store_dyn,
        runtime_store,
        blob_store,
    ));

    let storage = MobStorage::persistent(&mob_db_path).expect("create persistent mob storage");
    let handle = MobBuilder::new(gate_mob_definition(), storage)
        .with_session_service(service.clone())
        .with_default_llm_client(Arc::new(capture.clone()))
        .create()
        .await
        .expect("create persistent turn-latency mob");

    handle
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    handle
        .spawn_spec(SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from(SMALL_MEMBER_ID),
        ))
        .await
        .expect("spawn small worker");
    handle
        .spawn_spec(SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from(LARGE_MEMBER_ID),
        ))
        .await
        .expect("spawn large worker");

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

    // Seed transcripts. The lead gets a token turn; the SMALL member gets
    // its ~256 KB baseline document.
    handle
        .member(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead handle")
        .send(
            "fixture transcript for lead-1".to_string(),
            HandlingMode::Queue,
        )
        .await
        .expect("lead seed turn");
    let small_seed = "small baseline transcript "
        .repeat(SMALL_SEED_INPUT_BYTES / 24)
        .chars()
        .take(SMALL_SEED_INPUT_BYTES)
        .collect::<String>();
    handle
        .member(&AgentIdentity::from(SMALL_MEMBER_ID))
        .await
        .expect("small member handle")
        .send(small_seed, HandlingMode::Queue)
        .await
        .expect("small seed turn");
    wait_for_requests(&capture, 2, "lead + small seed turns").await;
    quiesce("after small seeding").await;
    let baseline_store_bytes = dir_size_bytes(&stores_root);

    // Grow ONE member to production scale (~10 MB of accumulated
    // transcript). Only the document size distinguishes the two fixtures.
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
    wait_for_requests(&capture, 2 + LARGE_SESSION_TURNS, "large seed turns").await;

    // The defect class is size-proportional durable-boundary work, so the
    // large document must actually be durable before anything is measured.
    let deadline = Instant::now() + Duration::from_secs(240);
    let grown_store_bytes = loop {
        let bytes = dir_size_bytes(&stores_root);
        if bytes >= baseline_store_bytes + MIN_LARGE_GROWTH_BYTES {
            break bytes;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the large session document to persist: \
             {bytes} bytes on disk (baseline {baseline_store_bytes}, need >= \
             {MIN_LARGE_GROWTH_BYTES} of growth)"
        );
        sleep(Duration::from_millis(250)).await;
    };
    eprintln!(
        "[turn-latency gate] durable stores: {:.2} MB after small seed, {:.2} MB after large growth",
        baseline_store_bytes as f64 / (1024.0 * 1024.0),
        grown_store_bytes as f64 / (1024.0 * 1024.0),
    );

    // Measure both fixtures in the same process, back to back: small first,
    // then large. Each window is quiesce-bracketed so trailing boundary
    // persistence is attributed to its own fixture.
    let small = measure_member_turns(&handle, &capture, SMALL_MEMBER_ID).await;
    eprintln!(
        "[turn-latency gate] small (~{} KB doc): {:?} CPU / {:?} wall per turn over {MEASURED_TURNS} turns",
        SMALL_SEED_INPUT_BYTES / 1024,
        small.cpu_per_turn,
        small.wall_per_turn,
    );
    let large = measure_member_turns(&handle, &capture, LARGE_MEMBER_ID).await;
    eprintln!(
        "[turn-latency gate] large (~{} MB doc): {:?} CPU / {:?} wall per turn over {MEASURED_TURNS} turns",
        (LARGE_TURN_INPUT_BYTES * LARGE_SESSION_TURNS) / (1024 * 1024),
        large.cpu_per_turn,
        large.wall_per_turn,
    );

    let cpu_ratio = large.cpu_per_turn.as_secs_f64() / small.cpu_per_turn.as_secs_f64().max(1e-9);
    let wall_ratio =
        large.wall_per_turn.as_secs_f64() / small.wall_per_turn.as_secs_f64().max(1e-9);
    let bytes_ratio =
        large.digest_bytes_per_turn as f64 / (small.digest_bytes_per_turn.max(1)) as f64;
    eprintln!(
        "[turn-latency gate] per-turn large/small: {bytes_ratio:.1}x bytes (ASSERTED), \
         {cpu_ratio:.1}x CPU (diagnostic), {wall_ratio:.1}x wall (diagnostic)",
    );

    // Denominator honesty (recalibrated 2026-07-27): the small-side digest
    // baseline may be ~0 (that IS the flat contract) but must stay under its
    // ceiling — a regressed or deliberately inflated baseline launders the
    // ratio. Instrument honesty rides the encode counter: each measured turn
    // must have produced at least one small document's worth of boundary
    // serialization, or the fixture drove no real boundary commits and no
    // digest conclusion means anything.
    assert!(
        small.digest_bytes_per_turn <= SMALL_DIGEST_BYTES_PER_TURN_MAX,
        "small-side digest baseline regressed: {} bytes hashed per turn at \
         the ~256 KB member (ceiling {SMALL_DIGEST_BYTES_PER_TURN_MAX}; large side \
         measured {} bytes per turn). Fix the baseline first, or recalibrate \
         deliberately with a comment",
        small.digest_bytes_per_turn,
        large.digest_bytes_per_turn,
    );
    assert!(
        small.encode_bytes_per_turn >= SMALL_ENCODE_BYTES_PER_TURN_MIN,
        "instrument honesty: the small member serialized only {} bytes per \
         measured turn (floor {SMALL_ENCODE_BYTES_PER_TURN_MIN}). The boundary \
         contract serializes the whole document once per commit, so this \
         fixture is not driving real boundary work and the flatness ratio \
         proves nothing",
        small.encode_bytes_per_turn,
    );

    // Boundary-serialization envelope backstop (per fixture, NOT a flatness
    // claim): the boundary contract still legitimately serializes the whole
    // document once per boundary commit, so encode bytes per turn are
    // O(document) by design — the one deliberately retained O(document)
    // axis (SessionDelta-style incremental persistence is tracked
    // separately). The envelope catches the pathological class on top of
    // that — per-message reserialize loops, repeated whole-document
    // persists. DIGEST flatness is asserted by the ratio gate in
    // `e2e_smoke_mob_turn_latency_gate`.
    let small_doc_bytes = SMALL_SEED_INPUT_BYTES as u64;
    let large_doc_bytes = (LARGE_TURN_INPUT_BYTES * LARGE_SESSION_TURNS) as u64;
    for (label, cost, doc_bytes) in [
        ("small", &small, small_doc_bytes),
        ("large", &large, large_doc_bytes),
    ] {
        // 6x headroom over the document: one boundary serialize plus JSON
        // expansion plus incidental snapshot writes (and an 8 MB floor so
        // the tiny fixture's fixed overheads never trip it). A
        // per-message-reserialize regression blows far past it.
        let envelope = doc_bytes.saturating_mul(6).max(8 * 1024 * 1024);
        assert!(
            cost.encode_bytes_per_turn <= envelope,
            "boundary serialization at the {label} member wrote {} bytes per \
             turn, above its per-fixture envelope of {envelope} (document \
             ~{doc_bytes} bytes). This is the repeated-reserialize backstop, \
             not a flatness gate — investigate what serializes the document \
             more than once per boundary",
            cost.encode_bytes_per_turn,
        );
    }

    handle.shutdown().await.expect("shutdown turn-latency mob");
    (small, large)
}

/// Smoke-lane gate: fixture validity, instrument honesty (small-side band),
/// the repeated-reserialize envelope, AND the flatness contract itself — an
/// identical tiny turn must hash the same bytes whether the accumulated
/// document is ~256 KB or ~10 MB. Any O(document) pass left on the ordinary
/// turn boundary (whole-document canonical serialize, whole-document digest,
/// full-snapshot decode, whole-blob rewrite) makes the large side track its
/// document size and trips the ratio. Armed in-lane with the 0.8.9
/// flat-curve work (witness-v3 revision-identity witness, producer-seeded
/// decode memo, seal-retyped save guards, framed checkpoint midstate,
/// compaction-commit digest reuse); the former `mob_turn_flatness_red_by_design`
/// out-of-lane split is retired.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_mob_turn_latency_gate() {
    let (small, large) = run_turn_latency_harness().await;
    let bytes_ratio =
        large.digest_bytes_per_turn as f64 / (small.digest_bytes_per_turn.max(1)) as f64;
    let flatness_budget = small
        .digest_bytes_per_turn
        .saturating_mul(MAX_LARGE_TO_SMALL_BYTES_RATIO)
        .saturating_add(LARGE_DIGEST_FIXED_ALLOWANCE_BYTES);
    assert!(
        large.digest_bytes_per_turn <= flatness_budget,
        "turn-boundary hashing scales with document size: {} bytes hashed \
         per turn at the ~10 MB member vs {} bytes at the ~256 KB member \
         ({bytes_ratio:.1}x; budget {MAX_LARGE_TO_SMALL_BYTES_RATIO}x + \
         {LARGE_DIGEST_FIXED_ALLOWANCE_BYTES} = {flatness_budget}; \
         diagnostics: CPU {:?} vs {:?}, wall {:?} vs {:?}). Turn-boundary \
         work must be O(delta), not O(document): a one-word reply on a large \
         session may not re-serialize, re-digest, or re-persist the whole \
         accumulated document",
        large.digest_bytes_per_turn,
        small.digest_bytes_per_turn,
        large.cpu_per_turn,
        small.cpu_per_turn,
        large.wall_per_turn,
        small.wall_per_turn,
    );
}
