#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! Ordinary-turn HeadCanonical storage-cost gate for the mob runtime
//! (turbo-s smoke lane), sibling to `smoke_mob_idle_burn`.
//!
//! The production defect this pins (measured 2026-07-25 on a live 0.8.6
//! fleet): identical one-word-ACK turns took 60 seconds at a 14 MB session
//! and over 180 seconds at 94 MB, because turn-boundary work — canonical
//! serialization + SHA-256 over the whole session document, authority
//! reloads that re-decode the full snapshot, and whole-blob persistence —
//! is O(document) regardless of how small the turn's actual delta is.
//!
//! The asserted contract is structural O(delta), not a calibrated large/small
//! ratio. `SqliteSessionStore` and `SqliteRuntimeStore` share one database and
//! the runtime explicitly selects `HeadCanonicalV1`. After both a ~256 KB and
//! a ~10 MB fixture are durable, identical tiny ordinary turns must:
//!
//! - perform zero whole-session encodes and only fixed delta-bounded content
//!   hashing;
//! - leave the whole-BLOB runtime snapshot table empty and retain only
//!   head-canonical runtime authority;
//! - append a fixed, tiny number and byte volume of canonical message rows;
//! - grow the co-tenant database plus WAL by a fixed delta-sized envelope.
//! - keep process CPU and wall time within a load-tolerant constant envelope
//!   relative to the identical small-document turn, catching uninstrumented
//!   scans, clones, and debug verification passes.
//!
//! Byte/row assertions are absolute and document-size independent. Timing is
//! a secondary bounded envelope with fixed slack, not the primary proof: it
//! covers work that has not yet reached one of the structural counters.
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
    AgentIdentity, MemberTurnOptions, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus,
    MobRuntimeMode, MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, SqliteSessionStore, StoreAdapter};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
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

/// Each scripted turn appends one tiny user row and one tiny assistant row.
/// Fixed headroom admits typed notices without making the bound depend on the
/// accumulated document.
const MAX_COMMITTED_SUFFIX_ROWS_PER_TURN: u64 = 8;
const MAX_COMMITTED_SUFFIX_BYTES_PER_TURN: u64 = 128 * 1024;
const MAX_DIGEST_BYTES_PER_TURN: u64 = 128 * 1024;

/// Absolute co-tenant SQLite growth envelope. This is intentionally far below
/// one copy of the ~10 MB fixture while leaving ample room for fixed-size
/// runtime receipt/lifecycle rows and SQLite page framing.
const MAX_DB_WAL_GROWTH_BYTES_PER_TURN: u64 = 2 * 1024 * 1024;

/// Secondary instrumentation-honesty envelope. Fixed slack absorbs scheduler
/// and SQLite variance while a 4x slope still rejects the measured hidden
/// O(document) witness verification (19.3x CPU / 6.1x wall).
const MAX_LARGE_SMALL_TIME_MULTIPLIER: f64 = 4.0;
const CPU_TIME_SLACK_PER_TURN: Duration = Duration::from_millis(500);
const WALL_TIME_SLACK_PER_TURN: Duration = Duration::from_secs(1);

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

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn file_size_bytes(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// Main database plus WAL high-water. The SHM file is a fixed coordination
/// artifact, not durable payload, and is deliberately excluded.
fn sqlite_db_and_wal_bytes(path: &Path) -> u64 {
    file_size_bytes(path).saturating_add(file_size_bytes(&sqlite_sidecar_path(path, "-wal")))
}

/// Start each measurement from a zero-length WAL so its final size is a useful
/// absolute write-growth witness instead of a high-water inherited from
/// fixture seeding or the previous member.
fn truncate_sqlite_wal(path: &Path) {
    let connection = Connection::open(path).expect("open co-tenant SQLite for WAL checkpoint");
    connection
        .busy_timeout(Duration::from_secs(30))
        .expect("configure checkpoint busy timeout");
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("read co-tenant SQLite journal mode");
    assert!(
        journal_mode.eq_ignore_ascii_case("wal"),
        "DB+WAL growth probe requires WAL journal mode, found {journal_mode}"
    );
    let (busy, log_frames, checkpointed_frames): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("truncate co-tenant SQLite WAL before measurement");
    assert_eq!(
        busy, 0,
        "co-tenant SQLite WAL checkpoint was blocked: \
         {log_frames} log frames / {checkpointed_frames} checkpointed"
    );
    drop(connection);
    assert_eq!(
        file_size_bytes(&sqlite_sidecar_path(path, "-wal")),
        0,
        "co-tenant SQLite WAL did not start the measurement at zero bytes"
    );
}

/// Hold a read snapshot across the window so SQLite cannot recycle/checkpoint
/// away frames before their high-water is observed. Ordinary writers remain
/// unblocked in WAL mode.
fn hold_sqlite_wal_snapshot(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("open co-tenant SQLite WAL probe");
    connection
        .busy_timeout(Duration::from_secs(30))
        .expect("configure WAL probe busy timeout");
    connection
        .execute_batch("BEGIN DEFERRED;")
        .expect("begin WAL probe read transaction");
    let _: i64 = connection
        .query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| row.get(0))
        .expect("establish WAL probe read snapshot");
    connection
}

#[derive(Debug, Clone, Copy)]
struct DurableStorageFacts {
    canonical_message_rows: u64,
    canonical_message_bytes: u64,
    whole_blob_snapshot_rows: u64,
    head_canonical_authority_rows: u64,
    session_head_rows: u64,
}

fn query_u64(connection: &Connection, sql: &str) -> u64 {
    let value: i64 = connection
        .query_row(sql, [], |row| row.get(0))
        .expect("read co-tenant SQLite storage-cost fact");
    u64::try_from(value).expect("SQLite storage-cost fact must be non-negative")
}

fn durable_storage_facts(path: &Path) -> DurableStorageFacts {
    let connection = Connection::open(path).expect("open co-tenant SQLite for storage facts");
    DurableStorageFacts {
        canonical_message_rows: query_u64(
            &connection,
            "SELECT COUNT(*) FROM session_strand_messages",
        ),
        canonical_message_bytes: query_u64(
            &connection,
            "SELECT COALESCE(SUM(length(message_json)), 0) FROM session_strand_messages",
        ),
        whole_blob_snapshot_rows: query_u64(
            &connection,
            "SELECT COUNT(*) FROM runtime_session_snapshots",
        ),
        head_canonical_authority_rows: query_u64(
            &connection,
            "SELECT COUNT(*) FROM runtime_session_authority",
        ),
        session_head_rows: query_u64(&connection, "SELECT COUNT(*) FROM session_heads"),
    }
}

fn checked_fact_delta(after: u64, before: u64, label: &str) -> u64 {
    after
        .checked_sub(before)
        .unwrap_or_else(|| panic!("co-tenant SQLite {label} retracted: {before} -> {after}"))
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
    /// Whole-document digest preimage bytes across the measurement window.
    digest_bytes: u64,
    /// Whole-session encode bytes across the measurement window.
    encode_bytes: u64,
    /// Newly committed canonical suffix rows across the window.
    committed_suffix_rows: u64,
    /// Serialized durable bytes in those suffix rows.
    committed_suffix_bytes: u64,
    /// Main co-tenant database + WAL size growth from a truncated-WAL start.
    db_wal_growth_bytes: u64,
    /// Runtime whole-BLOB snapshot rows after the window.
    whole_blob_snapshot_rows: u64,
    /// Runtime head-canonical authority rows after the window.
    head_canonical_authority_rows: u64,
    /// Canonical session-head rows after the window.
    session_head_rows: u64,
}

/// Drive `MEASURED_TURNS` identical tiny turns at one member and return the
/// per-turn process-CPU and wall cost. The window opens after a quiesce and
/// closes after the trailing quiesce, so asynchronous turn-boundary
/// persistence is attributed to the turn that caused it.
async fn measure_member_turns(
    handle: &MobHandle,
    capture: &CaptureClient,
    member_id: &str,
    realm_db_path: &Path,
) -> TurnCost {
    quiesce(&format!("before measuring {member_id}")).await;
    let member = handle
        .member(&AgentIdentity::from(member_id))
        .await
        .expect("measured member handle");

    truncate_sqlite_wal(realm_db_path);
    let durable_start = durable_storage_facts(realm_db_path);
    let db_wal_start = sqlite_db_and_wal_bytes(realm_db_path);
    let wal_growth_probe = hold_sqlite_wal_snapshot(realm_db_path);
    let cpu_start = process_cpu_time();
    let digest_bytes_start = meerkat_core::global_session_content_digest_bytes();
    let encode_bytes_start = meerkat_core::global_session_encode_bytes();
    let digest_sites_start = meerkat_core::digest_site_bytes();
    let wall_start = Instant::now();
    for turn in 0..MEASURED_TURNS {
        let expected = capture.count() + 1;
        member
            .start_turn(
                MEASURED_TURN_PROMPT.to_string(),
                HandlingMode::Queue,
                MemberTurnOptions::default(),
                None,
            )
            .await
            .expect("measured turn admission")
            .wait()
            .await
            .expect("measured turn committed completion");
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
    let digest_bytes =
        meerkat_core::global_session_content_digest_bytes().saturating_sub(digest_bytes_start);
    let encode_bytes =
        meerkat_core::global_session_encode_bytes().saturating_sub(encode_bytes_start);
    let durable_end = durable_storage_facts(realm_db_path);
    let db_wal_end = sqlite_db_and_wal_bytes(realm_db_path);
    drop(wal_growth_probe);
    let committed_suffix_rows = checked_fact_delta(
        durable_end.canonical_message_rows,
        durable_start.canonical_message_rows,
        "canonical message-row count",
    );
    let committed_suffix_bytes = checked_fact_delta(
        durable_end.canonical_message_bytes,
        durable_start.canonical_message_bytes,
        "canonical message-row bytes",
    );
    let db_wal_growth_bytes =
        checked_fact_delta(db_wal_end, db_wal_start, "main database + WAL bytes");
    eprintln!(
        "[turn-storage gate] {member_id}: {digest_bytes} whole-document digest bytes, \
         {encode_bytes} whole-session encode bytes over {MEASURED_TURNS} turns",
    );
    let digest_sites_end = meerkat_core::digest_site_bytes();
    for (index, label) in meerkat_core::DIGEST_SITE_LABELS.iter().enumerate() {
        let site_bytes = digest_sites_end[index].saturating_sub(digest_sites_start[index])
            / MEASURED_TURNS as u64;
        eprintln!(
            "[turn-storage gate]   {member_id} digest site {label}: {site_bytes} bytes per turn",
        );
    }

    eprintln!(
        "[turn-storage gate] {member_id}: {committed_suffix_rows} suffix rows / \
         {committed_suffix_bytes} suffix bytes / {db_wal_growth_bytes} DB+WAL \
         growth bytes over {MEASURED_TURNS} turns",
    );

    TurnCost {
        cpu_per_turn: cpu / MEASURED_TURNS as u32,
        wall_per_turn: wall / MEASURED_TURNS as u32,
        digest_bytes,
        encode_bytes,
        committed_suffix_rows,
        committed_suffix_bytes,
        db_wal_growth_bytes,
        whole_blob_snapshot_rows: durable_end.whole_blob_snapshot_rows,
        head_canonical_authority_rows: durable_end.head_canonical_authority_rows,
        session_head_rows: durable_end.session_head_rows,
    }
}

/// Shared harness: boots the 3-member mob, grows the fixtures, and asserts the
/// same absolute HeadCanonical O(delta) contract at both document sizes.
async fn run_turn_latency_harness() -> (TurnCost, TurnCost) {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    let user_config_root = root.join("user-config");
    let runtime_root = root.join("runtime-root");
    let project_root = root.join("project-root");
    let context_root = root.join("context-root");
    let stores_root = root.join("stores");
    let realm_db_path = stores_root.join("realm.db");
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
    // this gate targets the ORDINARY turn (exactly the production defect
    // shape: the live
    // 60 s/180 s one-word turns were not compacting). Raising the threshold
    // far above both fixtures makes both windows measure the identical
    // append-boundary operation. The compaction path's own ~6x redundancy
    // multiplier is pinned separately by the meerkat-core digest-budget and
    // rewrite-commit tests.
    config.compaction.auto_compact_threshold = 50_000_000;
    let mut builder = FactoryAgentBuilder::new(factory, config);
    // Production HomeCore shape: session rows and runtime authority share one
    // SQLite transaction domain, and the runtime profile is explicitly
    // HeadCanonical. Separate files would make an atomic O(delta) boundary
    // impossible and silently exercise the WholeBlob compatibility path.
    let store =
        Arc::new(SqliteSessionStore::open(&realm_db_path).expect("co-tenant SQLite session store"));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&realm_db_path)
            .expect("co-tenant HeadCanonical runtime store"),
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
        .start_turn(
            "fixture transcript for lead-1".to_string(),
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("lead seed turn admission")
        .wait()
        .await
        .expect("lead seed turn committed completion");
    let small_seed = "small baseline transcript "
        .repeat(SMALL_SEED_INPUT_BYTES / 24)
        .chars()
        .take(SMALL_SEED_INPUT_BYTES)
        .collect::<String>();
    handle
        .member(&AgentIdentity::from(SMALL_MEMBER_ID))
        .await
        .expect("small member handle")
        .start_turn(
            small_seed,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("small seed turn admission")
        .wait()
        .await
        .expect("small seed turn committed completion");
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
            .start_turn(
                filler,
                HandlingMode::Queue,
                MemberTurnOptions::default(),
                None,
            )
            .await
            .expect("large seed turn admission")
            .wait()
            .await
            .expect("large seed turn committed completion");
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
        "[turn-storage gate] durable stores: {:.2} MB after small seed, {:.2} MB after large growth",
        baseline_store_bytes as f64 / (1024.0 * 1024.0),
        grown_store_bytes as f64 / (1024.0 * 1024.0),
    );

    // Measure both fixtures in the same process, back to back: small first,
    // then large. Each window is quiesce-bracketed so trailing boundary
    // persistence is attributed to its own fixture.
    let small = measure_member_turns(&handle, &capture, SMALL_MEMBER_ID, &realm_db_path).await;
    eprintln!(
        "[turn-storage gate] small (~{} KB doc): {:?} CPU / {:?} wall per turn over {MEASURED_TURNS} turns",
        SMALL_SEED_INPUT_BYTES / 1024,
        small.cpu_per_turn,
        small.wall_per_turn,
    );
    let large = measure_member_turns(&handle, &capture, LARGE_MEMBER_ID, &realm_db_path).await;
    eprintln!(
        "[turn-storage gate] large (~{} MB doc): {:?} CPU / {:?} wall per turn over {MEASURED_TURNS} turns",
        (LARGE_TURN_INPUT_BYTES * LARGE_SESSION_TURNS) / (1024 * 1024),
        large.cpu_per_turn,
        large.wall_per_turn,
    );

    let cpu_ratio = large.cpu_per_turn.as_secs_f64() / small.cpu_per_turn.as_secs_f64().max(1e-9);
    let wall_ratio =
        large.wall_per_turn.as_secs_f64() / small.wall_per_turn.as_secs_f64().max(1e-9);
    eprintln!(
        "[turn-storage gate] per-turn large/small: {cpu_ratio:.1}x CPU / \
         {wall_ratio:.1}x wall",
    );
    let maximum_large_cpu = small
        .cpu_per_turn
        .mul_f64(MAX_LARGE_SMALL_TIME_MULTIPLIER)
        .saturating_add(CPU_TIME_SLACK_PER_TURN);
    let maximum_large_wall = small
        .wall_per_turn
        .mul_f64(MAX_LARGE_SMALL_TIME_MULTIPLIER)
        .saturating_add(WALL_TIME_SLACK_PER_TURN);
    assert!(
        large.cpu_per_turn <= maximum_large_cpu,
        "ordinary HeadCanonical large-document turns consumed {:?} CPU per turn, above the \
         load-tolerant {:?} envelope derived from identical small-document turns; an \
         uninstrumented O(document) scan/clone/verification pass remains",
        large.cpu_per_turn,
        maximum_large_cpu,
    );
    assert!(
        large.wall_per_turn <= maximum_large_wall,
        "ordinary HeadCanonical large-document turns consumed {:?} wall time per turn, above the \
         load-tolerant {:?} envelope derived from identical small-document turns; an \
         uninstrumented O(document) wait or blocking pass remains",
        large.wall_per_turn,
        maximum_large_wall,
    );

    for (label, cost) in [("small", &small), ("large", &large)] {
        let maximum_digest_bytes =
            (MEASURED_TURNS as u64).saturating_mul(MAX_DIGEST_BYTES_PER_TURN);
        assert!(
            cost.digest_bytes <= maximum_digest_bytes,
            "ordinary HeadCanonical turns at the {label} member hashed {} \
             content-digest bytes, above the fixed {maximum_digest_bytes}-byte \
             delta envelope; this path must use retained digest/row-prefix \
             authority without a document-sized verification pass",
            cost.digest_bytes,
        );
        assert_eq!(
            cost.encode_bytes, 0,
            "ordinary HeadCanonical turns at the {label} member encoded {} \
             whole-session bytes; the typed prepared boundary must have no \
             WholeBlob representation or fallback",
            cost.encode_bytes,
        );
        assert_eq!(
            cost.whole_blob_snapshot_rows, 0,
            "ordinary HeadCanonical turns at the {label} member left {} \
             runtime whole-BLOB snapshot rows in the fresh co-tenant realm",
            cost.whole_blob_snapshot_rows,
        );
        assert!(
            cost.head_canonical_authority_rows >= MEMBER_IDS.len() as u64,
            "ordinary HeadCanonical turns at the {label} member expose only {} \
             head-canonical runtime authorities for {} active members",
            cost.head_canonical_authority_rows,
            MEMBER_IDS.len(),
        );
        assert!(
            cost.session_head_rows >= MEMBER_IDS.len() as u64,
            "ordinary HeadCanonical turns at the {label} member expose only {} \
             canonical session heads for {} active members",
            cost.session_head_rows,
            MEMBER_IDS.len(),
        );

        let minimum_suffix_rows = (MEASURED_TURNS as u64).saturating_mul(2);
        let maximum_suffix_rows =
            (MEASURED_TURNS as u64).saturating_mul(MAX_COMMITTED_SUFFIX_ROWS_PER_TURN);
        assert!(
            cost.committed_suffix_rows >= minimum_suffix_rows,
            "instrument honesty: {MEASURED_TURNS} completed turns at the {label} \
             member committed only {} canonical suffix rows (expected at least \
             one user + one assistant row per turn)",
            cost.committed_suffix_rows,
        );
        assert!(
            cost.committed_suffix_rows <= maximum_suffix_rows,
            "ordinary HeadCanonical turns at the {label} member committed {} \
             canonical suffix rows, above the fixed {maximum_suffix_rows}-row \
             envelope",
            cost.committed_suffix_rows,
        );
        let maximum_suffix_bytes =
            (MEASURED_TURNS as u64).saturating_mul(MAX_COMMITTED_SUFFIX_BYTES_PER_TURN);
        assert!(
            cost.committed_suffix_bytes > 0 && cost.committed_suffix_bytes <= maximum_suffix_bytes,
            "ordinary HeadCanonical turns at the {label} member committed {} \
             canonical suffix bytes; expected a non-empty delta no larger than \
             the fixed {maximum_suffix_bytes}-byte envelope",
            cost.committed_suffix_bytes,
        );
        let maximum_db_wal_growth =
            (MEASURED_TURNS as u64).saturating_mul(MAX_DB_WAL_GROWTH_BYTES_PER_TURN);
        assert!(
            cost.db_wal_growth_bytes <= maximum_db_wal_growth,
            "ordinary HeadCanonical turns at the {label} member grew the \
             co-tenant database + WAL by {} bytes, above the fixed \
             {maximum_db_wal_growth}-byte delta envelope",
            cost.db_wal_growth_bytes,
        );
    }

    handle.shutdown().await.expect("shutdown turn-storage mob");
    (small, large)
}

/// Smoke-lane gate for the real shared-file HeadCanonical boundary.
///
/// At both fixture sizes it asserts fixed delta-bounded digest bytes, zero
/// whole-session encode bytes, zero durable whole-BLOB snapshots,
/// head-canonical authority presence, bounded committed suffix rows/bytes,
/// and bounded main-DB + WAL growth. CPU and wall remain diagnostic only.
///
/// Two dynamic blind spots remain explicit because production exposes no
/// process-wide probe for them: a read-only attempt to consult a whole-BLOB
/// fallback, and full `Session`/history clone or materialization counts. The
/// disjoint HeadCanonical boundary carrier makes the former a typed error;
/// this gate additionally proves that no fallback document is encoded or
/// durably written.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_mob_turn_head_canonical_storage_cost_gate() {
    let _ = run_turn_latency_harness().await;
}
