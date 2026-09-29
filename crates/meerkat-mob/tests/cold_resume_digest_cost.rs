//! Cold-resume content-digest cost gate.
//!
//! The production defect this pins (HomeCore, 18-member mob, 2026-09): after a
//! cold restart the mob spent tens of minutes single-threaded in canonical
//! JSON plus SHA-256 over whole transcripts (8.8k-12k messages per member)
//! before it could serve anything, and the first turn of a large member took
//! minutes more. Every durable transcript was already verified by the store
//! authority at materialization; recomputing whole-document digests for the
//! same bytes on resume and again on the first turn is redundant work.
//!
//! The harness boots a persistent mob on the production realm shape (SQLite
//! session rows and HeadCanonical runtime authority co-tenant in one
//! database), grows members to transcripts of many messages with tool
//! rounds, cold-stops it, resumes it through `MobBuilder::for_resume`, and
//! drives one turn at each member. It reports the content-digest bytes per
//! digest site for the resume window and for the first-turn window, and
//! asserts they stay within a small constant multiple of the transcripts'
//! own canonical size.
//!
//! Scale is adjustable for measurement:
//!   MEERKAT_COLD_RESUME_TURNS=<n> MEERKAT_COLD_RESUME_TOOL_ROUNDS=<n>
//!
//! The digest counters are process-global, and the verification they count
//! runs on blocking-pool threads, so per-thread counters cannot attribute it.
//! Nextest runs each test in its own process; under plain `cargo test` the
//! harnesses in this binary share one process, so each holds
//! [`DIGEST_WINDOW_SERIAL`] for its whole run and no other harness can move
//! the counters inside its measured windows.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_core::types::HandlingMode;
use meerkat_mob::definition::{OrchestratorConfig, WiringRules};
use meerkat_mob::{
    AgentIdentity, MemberTurnOptions, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus,
    MobRuntimeMode, MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, SqliteSessionStore, StoreAdapter};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{Duration, Instant};

const LEAD_ID: &str = "lead-1";
const LARGE_MEMBER_IDS: [&str; 2] = ["w-large-a", "w-large-b"];

/// Default fixture scale: each seed turn performs this many tool rounds, so
/// one seed turn appends `2 * TOOL_ROUNDS + 2` messages.
const DEFAULT_SEED_TURNS: usize = 6;

/// Far above every fixture, so only the seeding lifetime of the compacting
/// fixture ever compacts.
const NO_COMPACTION_THRESHOLD: u64 = 50_000_000;
const DEFAULT_TOOL_ROUNDS: usize = 30;

/// Filler carried by every scripted assistant block, so each message has a
/// realistic body instead of a two-byte ACK.
const ASSISTANT_FILLER_BYTES: usize = 512;

/// Resume verifies each committed head against its store authority, which
/// hashes every durable message row about once. Relative to the verified
/// bytes (live transcripts plus retained pre-rewrite anchors), resume hashed
/// 4.53x for append-only heads and 7.19x for compacted heads before #1258,
/// and 0.91x and 0.97x after it (the CHANGELOG entry for #1258). Each large
/// member holds a bit under half of the verified bytes, so a second
/// materialization of any large head adds about 0.45x and exceeds this bound.
const MAX_RESUME_DIGEST_MULTIPLE: f64 = 1.25;

/// A single-shot turn after a verified resume must reuse the digest state the
/// verification seeded; it hashes its own two-message delta and fixed-size
/// metadata only. Re-seeding either large transcript alone exceeds this.
const MAX_FIRST_TURN_DIGEST_MULTIPLE: f64 = 0.1;

/// Serializes the harnesses of this binary so that each owns the
/// process-global digest counters for its measured windows (see the module
/// documentation).
static DIGEST_WINDOW_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// Scripted provider: every turn performs `tool_rounds` rounds of one
/// `peers` call with a filler text block, then ends with a short answer.
#[derive(Clone)]
struct ToolRoundClient {
    requests: Arc<AtomicUsize>,
    tool_rounds: Arc<AtomicUsize>,
}

impl ToolRoundClient {
    fn new(tool_rounds: usize) -> Self {
        Self {
            requests: Arc::new(AtomicUsize::new(0)),
            tool_rounds: Arc::new(AtomicUsize::new(tool_rounds)),
        }
    }

    /// Later turns answer directly, so a measured turn appends a two-message
    /// delta and its digest cost is attributable to retained state.
    fn stop_tool_rounds(&self) {
        self.tool_rounds.store(0, Ordering::SeqCst);
    }
}

fn tool_rounds_in_current_turn(messages: &[meerkat_core::Message]) -> usize {
    messages
        .iter()
        .rev()
        .take_while(|message| !matches!(message, meerkat_core::Message::User(_)))
        .filter(|message| matches!(message, meerkat_core::Message::ToolResults { .. }))
        .count()
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for ToolRoundClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let request_index = self.requests.fetch_add(1, Ordering::SeqCst);
        let round = tool_rounds_in_current_turn(&request.messages);
        let filler = format!("round {round} of request {request_index}: ")
            .repeat(ASSISTANT_FILLER_BYTES / 16)
            .chars()
            .take(ASSISTANT_FILLER_BYTES)
            .collect::<String>();
        let mut events = vec![meerkat_client::LlmEvent::TextDelta {
            delta: filler,
            meta: None,
        }];
        // Compaction summary requests carry no tools; answer them with text.
        let stop_reason =
            if round < self.tool_rounds.load(Ordering::SeqCst) && !request.tools.is_empty() {
                events.push(meerkat_client::LlmEvent::ToolCallComplete {
                    id: format!("call-{request_index}"),
                    name: "peers".to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                });
                meerkat_core::StopReason::ToolUse
            } else {
                meerkat_core::StopReason::EndTurn
            };
        events.push(meerkat_client::LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        });
        events.push(meerkat_client::LlmEvent::Done {
            outcome: meerkat_client::LlmDoneOutcome::Success { stop_reason },
        });
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

fn profile(peer_description: &str) -> Profile {
    Profile {
        model_fallback: None,
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

fn mob_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        ProfileBinding::Inline(Box::new(profile("Leads the cold-resume cost mob"))),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(profile("Cold-resume cost worker"))),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "cold-resume-digest-{}",
        meerkat_core::time_compat::new_uuid_v7()
    )));
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

struct Paths {
    root: PathBuf,
    realm_db_path: PathBuf,
    mob_db_path: PathBuf,
}

impl Paths {
    fn new(root: &Path) -> Self {
        let paths = Self {
            root: root.to_path_buf(),
            realm_db_path: root.join("stores").join("realm.db"),
            mob_db_path: root.join("mob.db"),
        };
        for dir in [
            root.join("project-root"),
            root.join("context-root"),
            root.join("stores"),
        ] {
            fs::create_dir_all(dir).expect("create cold-resume roots");
        }
        paths
    }
}

/// Production realm shape: SQLite session rows and HeadCanonical runtime
/// authority share one database.
fn persistent_service(
    paths: &Paths,
    client: &ToolRoundClient,
    auto_compact_threshold: u64,
) -> Arc<PersistentSessionService<FactoryAgentBuilder>> {
    let factory = AgentFactory::new(paths.root.join("runtime-root").join("factory-store"))
        .user_config_root(paths.root.join("user-config"))
        .runtime_root(paths.root.join("runtime-root"))
        .project_root(paths.root.join("project-root"))
        .context_root(paths.root.join("context-root"))
        .builtins(true)
        .comms(true);
    let mut config = Config::default();
    config.compaction.auto_compact_threshold = auto_compact_threshold;
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(Arc::new(client.clone()));
    let store = Arc::new(
        SqliteSessionStore::open(&paths.realm_db_path).expect("co-tenant SQLite session store"),
    );
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));
    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&paths.realm_db_path)
            .expect("co-tenant HeadCanonical runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::default());
    Arc::new(PersistentSessionService::new(
        builder,
        32,
        store_dyn,
        runtime_store,
        blob_store,
    ))
}

/// Settles on the mob actor's own machine-state publications: the member
/// status projection is re-read after every published change, never on a
/// timer. The deadline only bounds a broken run, so it fails with the roster
/// instead of hanging until the harness timeout.
async fn wait_all_active(handle: &MobHandle, expected: usize, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut changes = handle.machine_state_changes();
    loop {
        let members = handle.list_members().await;
        let active = members
            .iter()
            .filter(|entry| entry.status == MobMemberStatus::Active)
            .count();
        if active >= expected {
            return;
        }
        tokio::select! {
            changed = changes.changed() => changed.unwrap_or_else(|_| {
                panic!(
                    "the mob actor stopped before {expected} members were active {what}; \
                     roster: {members:?}"
                )
            }),
            () = tokio::time::sleep_until(deadline) => panic!(
                "timed out waiting for {expected} active members {what}; roster: {members:?}"
            ),
        }
    }
}

async fn run_turn(handle: &MobHandle, member_id: &str, prompt: String) {
    handle
        .member(&AgentIdentity::from(member_id))
        .await
        .expect("member handle")
        .start_turn(
            prompt,
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("turn admission")
        .wait()
        .await
        .expect("turn committed completion");
}

#[derive(Debug, Clone, Copy)]
struct DigestMark {
    total: u64,
    sites: [u64; 6],
    encode: u64,
    body_decodes: u64,
}

impl DigestMark {
    fn now() -> Self {
        Self {
            total: meerkat_core::global_session_content_digest_bytes(),
            sites: meerkat_core::digest_site_bytes(),
            encode: meerkat_core::global_session_encode_bytes(),
            body_decodes: meerkat_core::rewrite_record_body_decodes(),
        }
    }

    fn since(self, start: Self) -> Self {
        Self {
            total: self.total.saturating_sub(start.total),
            sites: std::array::from_fn(|site| self.sites[site].saturating_sub(start.sites[site])),
            encode: self.encode.saturating_sub(start.encode),
            body_decodes: self.body_decodes.saturating_sub(start.body_decodes),
        }
    }

    fn report(&self, window: &str, verified_bytes: u64) {
        eprintln!(
            "[cold-resume digest] {window}: {} digest bytes ({:.2}x the {verified_bytes} \
             verified transcript bytes), {} whole-session encode bytes, {} rewrite-record \
             body decodes",
            self.total,
            self.total as f64 / verified_bytes.max(1) as f64,
            self.encode,
            self.body_decodes,
        );
        for (index, label) in meerkat_core::DIGEST_SITE_LABELS.iter().enumerate() {
            eprintln!(
                "[cold-resume digest]   {window} site {label}: {} bytes",
                self.sites[index]
            );
        }
    }
}

/// Scenario shape: plain append-only transcripts, or transcripts that
/// auto-compact during seeding so resume materializes rewritten heads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fixture {
    AppendOnly,
    Compacting,
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_resume_digest_bytes_stay_proportional_to_transcripts() {
    run_cold_resume_harness(Fixture::AppendOnly).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_resume_of_compacted_transcripts_digest_bytes_stay_proportional() {
    run_cold_resume_harness(Fixture::Compacting).await;
}

async fn run_cold_resume_harness(fixture: Fixture) {
    let _digest_window = DIGEST_WINDOW_SERIAL.lock().await;
    let seed_turns = env_usize("MEERKAT_COLD_RESUME_TURNS", DEFAULT_SEED_TURNS);
    let tool_rounds = env_usize("MEERKAT_COLD_RESUME_TOOL_ROUNDS", DEFAULT_TOOL_ROUNDS);
    let auto_compact_threshold = match fixture {
        Fixture::AppendOnly => NO_COMPACTION_THRESHOLD,
        // Roughly every other seed turn crosses this estimated-history
        // threshold, so each large member carries several rewrites.
        Fixture::Compacting => env_usize("MEERKAT_COLD_RESUME_COMPACT_THRESHOLD", 30_000) as u64,
    };
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let client = ToolRoundClient::new(tool_rounds);

    // ---------------- Lifetime 1: grow large transcripts ----------------
    let service_1 = persistent_service(&paths, &client, auto_compact_threshold);
    let handle_1 = MobBuilder::new(
        mob_definition(),
        MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage"),
    )
    .with_session_service(service_1.clone())
    .with_default_llm_client(Arc::new(client.clone()))
    .create()
    .await
    .expect("create persistent mob");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from(LEAD_ID)))
        .await
        .expect("spawn lead");
    for member in LARGE_MEMBER_IDS {
        handle_1
            .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from(member)))
            .await
            .expect("spawn large worker");
    }
    let member_count = 1 + LARGE_MEMBER_IDS.len();
    wait_all_active(&handle_1, member_count, "before seeding").await;

    let seed_started = Instant::now();
    for member in LARGE_MEMBER_IDS {
        for turn in 0..seed_turns {
            run_turn(&handle_1, member, format!("seed turn {turn} for {member}")).await;
        }
    }
    eprintln!(
        "[cold-resume digest] seeded {} members x {seed_turns} turns x {tool_rounds} tool rounds \
         in {:?}",
        LARGE_MEMBER_IDS.len(),
        seed_started.elapsed()
    );

    let mut session_ids = Vec::new();
    let mut transcript_bytes = 0u64;
    let mut transcript_messages = 0usize;
    let mut rewritten_members = 0usize;
    let mut verified_bytes = 0u64;
    for member in LARGE_MEMBER_IDS.into_iter().chain([LEAD_ID]) {
        let session_id = handle_1
            .resolve_bridge_session_id(&AgentIdentity::from(member))
            .await
            .expect("member session id");
        let session = service_1
            .load_authoritative_session(&session_id)
            .await
            .expect("load member session")
            .expect("member session exists");
        let bytes = serde_json::to_vec(session.messages())
            .expect("serialize transcript")
            .len() as u64;
        let history = session
            .validated_transcript_history_state()
            .expect("validated transcript history");
        let rewrites = history
            .as_ref()
            .map_or(0, |history| history.rewrite_prefix().occurrence_count());
        // A rewritten head also retains its pre-rewrite anchor transcript,
        // whose content digest a cold read proves once against the graph.
        let anchor_bytes = history.as_ref().map_or(0, |history| {
            serde_json::to_vec(history.anchor().messages())
                .expect("serialize retained anchor")
                .len() as u64
        });
        if rewrites > 0 {
            rewritten_members += 1;
        }
        eprintln!(
            "[cold-resume digest] {member}: {} messages, {bytes} transcript bytes, {rewrites} \
             rewrites, {anchor_bytes} retained anchor bytes",
            session.messages().len()
        );
        transcript_bytes += bytes;
        verified_bytes += bytes + anchor_bytes;
        transcript_messages += session.messages().len();
        session_ids.push(session_id);
    }
    match fixture {
        Fixture::AppendOnly => assert!(
            transcript_messages >= LARGE_MEMBER_IDS.len() * seed_turns * (2 * tool_rounds + 2),
            "instrument honesty: fixture grew only {transcript_messages} messages"
        ),
        Fixture::Compacting => assert!(
            rewritten_members == LARGE_MEMBER_IDS.len(),
            "instrument honesty: only {rewritten_members} large members compacted"
        ),
    }

    // Digesting every live transcript and every retained anchor once is the
    // legitimate floor of verifying the committed heads on a cold read.
    eprintln!(
        "[cold-resume digest] {verified_bytes} verified transcript bytes \
         ({transcript_bytes} live)"
    );

    handle_1.shutdown().await.expect("shutdown before restart");
    for session_id in &session_ids {
        service_1
            .discard_live_session(session_id)
            .await
            .expect("discard live session before cold restart");
    }
    drop(handle_1);
    drop(service_1);

    // ---------------- Lifetime 2: cold resume ----------------
    // The resumed lifetime never compacts: a compaction turn legitimately
    // hashes the whole rebuilt transcript, and this gate measures what resume
    // leaves for the next ordinary turn.
    let service_2 = persistent_service(&paths, &client, NO_COMPACTION_THRESHOLD);
    let resume_start = DigestMark::now();
    let resume_clock = Instant::now();
    let handle_2 = MobBuilder::for_resume(
        MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage"),
    )
    .with_session_service(service_2.clone())
    .with_default_llm_client(Arc::new(client.clone()))
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("mob resume after cold restart");
    wait_all_active(&handle_2, member_count, "after resume").await;
    let resume = DigestMark::now().since(resume_start);
    eprintln!(
        "[cold-resume digest] resume to all-active: {:?}",
        resume_clock.elapsed()
    );
    resume.report(&format!("{fixture:?} resume"), verified_bytes);

    // ---------------- First turn after resume ----------------
    client.stop_tool_rounds();
    let first_turn_start = DigestMark::now();
    let first_turn_clock = Instant::now();
    let requests_before = client.requests.load(Ordering::SeqCst);
    for member in LARGE_MEMBER_IDS {
        run_turn(
            &handle_2,
            member,
            format!("first turn after resume for {member}"),
        )
        .await;
    }
    assert!(
        client.requests.load(Ordering::SeqCst) > requests_before,
        "instrument honesty: first turns after resume reached no provider"
    );
    let first_turn = DigestMark::now().since(first_turn_start);
    eprintln!(
        "[cold-resume digest] first turns: {:?}",
        first_turn_clock.elapsed()
    );
    first_turn.report(&format!("{fixture:?} first-turn"), verified_bytes);

    handle_2.shutdown().await.expect("final shutdown");

    let resume_budget = (verified_bytes as f64 * MAX_RESUME_DIGEST_MULTIPLE) as u64;
    assert!(
        resume.total <= resume_budget,
        "cold resume hashed {} content-digest bytes for {verified_bytes} verified \
         transcript bytes (budget {resume_budget}); a committed head is materialized and \
         verified more than once per resume",
        resume.total,
    );
    let first_turn_budget = (verified_bytes as f64 * MAX_FIRST_TURN_DIGEST_MULTIPLE) as u64;
    assert!(
        first_turn.total <= first_turn_budget,
        "the first turns after a cold resume hashed {} content-digest bytes for \
         {verified_bytes} verified transcript bytes (budget {first_turn_budget}); the \
         verified resume must seed the retained digest state",
        first_turn.total,
    );
}
