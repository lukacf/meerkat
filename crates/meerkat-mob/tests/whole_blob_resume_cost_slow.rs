//! Cold-resume cost gate for the WholeBlob runtime profile with deep rewrite
//! history.
//!
//! A production deployment runs the WholeBlob runtime store: every committed
//! session is one serialized document, and members carry long compaction
//! histories (rewrite generation ~119). On a cold resume every consumer that
//! needed the committed body decoded the whole document again, re-validated
//! its rewrite graph and re-digested its transcript: resume preparation,
//! actor creation, operation-binding restore, cleanup-composition restore and
//! the compaction-outbox checkpoint each paid a full decode.
//!
//! The harness grows members through real turns, then replaces each large
//! member's committed snapshot with a synthesized transcript carrying many
//! rewrite generations (a direct WholeBlob store fixture, so no compaction
//! turn runs), cold-stops, resumes through `MobBuilder::for_resume`, and
//! drives one turn per member. It reports whole-document decodes, decoded
//! bytes, rewrite-graph validations and content-digest bytes for the resume
//! window and the first-turn window.
//!
//! It then drives two steady-state turns per member (whose committed heads
//! were written by per-turn runtime boundaries the session service never
//! saw) and checks that the service's verified-body cache retains nothing
//! past the resume window.
//!
//! Multi-second, hence the `_slow` binary name. Which lane enforces it:
//! Cargo's `fast` nextest profile (`make test`, `make test-int`, and the
//! nightly `workspace-int` lane) still runs this binary, because that profile
//! excludes only the dedicated e2e binaries. Only Bazel's `fast_tests` suite
//! skips it, since the Bazel generator tags it `slow`. PR CI runs
//! `--lib --bins` only, so like every integration test it runs nightly.
//!
//! Scale: MEERKAT_WHOLE_BLOB_GENERATIONS, MEERKAT_WHOLE_BLOB_GENERATION_MESSAGES.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_core::types::HandlingMode;
use meerkat_mob::definition::{OrchestratorConfig, WiringRules};
use meerkat_mob::{
    AgentIdentity, MemberTurnOptions, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus,
    MobRuntimeMode, MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_runtime::RuntimeStore as _;
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, SqliteSessionStore, StoreAdapter};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{Duration, Instant, sleep};

const LEAD_ID: &str = "lead-1";
const LARGE_MEMBER_IDS: [&str; 2] = ["w-large-a", "w-large-b"];
const DEFAULT_GENERATIONS: usize = 60;
const DEFAULT_GENERATION_MESSAGES: usize = 16;
const MESSAGE_FILLER_BYTES: usize = 1024;
const NO_COMPACTION_THRESHOLD: u64 = 50_000_000;

/// Resume decodes each committed document once, for the store-owned
/// durable-tail recovery. Every later consumer reuses that verified body, and
/// the startup compaction-outbox refresh re-commits bytes the store already
/// holds a verified session for. Measured before the reuse: 8 decodes and 8
/// rewrite-graph validations per rewritten member (60 and 120 generations
/// alike).
///
/// Exact fit on purpose: decode and graph-validation counts are deterministic
/// functions of the code path (no timing, no retries on this fixture), so any
/// headroom would let one redundant consumer back in unnoticed. The digest
/// multiples below are byte ratios and carry headroom.
const MAX_RESUME_DECODES_PER_SESSION: u64 = 1;

/// Digest bytes at resume relative to the committed documents: one decode's
/// graph replay plus the create-time encode audit. Measured 8.70x before,
/// 1.89x after.
const MAX_RESUME_DIGEST_MULTIPLE: f64 = 2.25;

/// Turns per member after the first, measured as the steady state.
const STEADY_TURNS_PER_MEMBER: usize = 2;

/// Digest bytes for the first turn after resume: the turn commit's own encode
/// audit, with no decode. Measured 2.90x before, 0.90x after.
const MAX_FIRST_TURN_DIGEST_MULTIPLE: f64 = 1.25;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Default)]
struct TextClient {
    requests: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for TextClient {
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
        self.requests.fetch_add(1, Ordering::SeqCst);
        let events = vec![
            meerkat_client::LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            },
            meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
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
        ProfileBinding::Inline(Box::new(profile("Leads the WholeBlob resume mob"))),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(profile("WholeBlob resume worker"))),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "whole-blob-resume-{}",
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
    runtime_db: PathBuf,
    sessions_db: PathBuf,
    mob_db: PathBuf,
}

impl Paths {
    fn new(root: &Path) -> Self {
        for dir in [root.join("project-root"), root.join("context-root")] {
            fs::create_dir_all(dir).expect("create roots");
        }
        Self {
            root: root.to_path_buf(),
            runtime_db: root.join("runtime.sqlite"),
            sessions_db: root.join("sessions.sqlite"),
            mob_db: root.join("mob.db"),
        }
    }
}

/// Production WholeBlob shape: an independent WholeBlob runtime store file
/// holds session authority; the session store is a separate file.
fn persistent_service(
    paths: &Paths,
    client: &TextClient,
) -> Arc<PersistentSessionService<FactoryAgentBuilder>> {
    let factory = AgentFactory::new(paths.root.join("runtime-root").join("factory-store"))
        .user_config_root(paths.root.join("user-config"))
        .runtime_root(paths.root.join("runtime-root"))
        .project_root(paths.root.join("project-root"))
        .context_root(paths.root.join("context-root"))
        .builtins(true)
        .comms(true);
    let mut config = Config::default();
    config.compaction.auto_compact_threshold = NO_COMPACTION_THRESHOLD;
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(Arc::new(client.clone()));
    let store = Arc::new(SqliteSessionStore::open(&paths.sessions_db).expect("session store"));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));
    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_whole_blob(&paths.runtime_db)
            .expect("WholeBlob runtime store"),
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

async fn wait_all_active(handle: &MobHandle, expected: usize, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let members = handle.list_members().await;
        let active = members
            .iter()
            .filter(|entry| entry.status == MobMemberStatus::Active)
            .count();
        if active >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} active members {what}; roster: {members:?}"
        );
        sleep(Duration::from_millis(50)).await;
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

fn filler(label: &str) -> String {
    format!("{label} ")
        .repeat(MESSAGE_FILLER_BYTES / (label.len() + 1) + 1)
        .chars()
        .take(MESSAGE_FILLER_BYTES)
        .collect()
}

/// Grow `session` by `generations` compaction-shaped rewrites: each
/// generation appends `messages` user/assistant rows and then rewrites all
/// but the first and last rows into one summary.
fn synthesize_rewrite_history(
    session: &mut meerkat_core::Session,
    generations: usize,
    messages: usize,
) {
    for generation in 0..generations {
        for index in 0..messages {
            session.push(meerkat_core::Message::User(
                meerkat_core::types::UserMessage::text(filler(&format!(
                    "generation {generation} message {index}"
                ))),
            ));
        }
        let end = session.messages().len() - 1;
        let parent_revision = session.transcript_revision().expect("parent revision");
        session
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 1, end },
                vec![meerkat_core::Message::User(
                    meerkat_core::types::UserMessage::text(format!(
                        "[summary of generation {generation}] {}",
                        filler("summary")
                    )),
                )],
                meerkat_core::TranscriptRewriteReason::new("synthetic compaction"),
                Some(format!("whole-blob-fixture-{generation}")),
                Some(parent_revision),
            )
            .expect("synthetic rewrite");
    }
    for index in 0..messages {
        session.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text(filler(&format!("live tail {index}"))),
        ));
    }
}

#[derive(Debug, Clone, Copy)]
struct CostMark {
    decodes: u64,
    decode_bytes: u64,
    graph_validations: u64,
    digest_bytes: u64,
    encode_bytes: u64,
}

impl CostMark {
    fn now() -> Self {
        Self {
            decodes: meerkat_core::global_whole_blob_decodes(),
            decode_bytes: meerkat_core::global_whole_blob_decode_bytes(),
            graph_validations: meerkat_core::global_transcript_graph_validations(),
            digest_bytes: meerkat_core::global_session_content_digest_bytes(),
            encode_bytes: meerkat_core::global_session_encode_bytes(),
        }
    }

    fn since(self, start: Self) -> Self {
        Self {
            decodes: self.decodes - start.decodes,
            decode_bytes: self.decode_bytes - start.decode_bytes,
            graph_validations: self.graph_validations - start.graph_validations,
            digest_bytes: self.digest_bytes - start.digest_bytes,
            encode_bytes: self.encode_bytes - start.encode_bytes,
        }
    }

    fn report(&self, window: &str, sessions: usize, document_bytes: u64) {
        eprintln!(
            "[whole-blob resume] {window}: {} decodes ({:.2} per session), {} decoded bytes \
             ({:.2}x of {document_bytes} document bytes), {} graph validations, {} digest \
             bytes ({:.2}x), {} encode bytes",
            self.decodes,
            self.decodes as f64 / sessions as f64,
            self.decode_bytes,
            self.decode_bytes as f64 / document_bytes.max(1) as f64,
            self.graph_validations,
            self.digest_bytes,
            self.digest_bytes as f64 / document_bytes.max(1) as f64,
            self.encode_bytes,
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn whole_blob_cold_resume_with_deep_rewrite_history() {
    let generations = env_usize("MEERKAT_WHOLE_BLOB_GENERATIONS", DEFAULT_GENERATIONS);
    let generation_messages = env_usize(
        "MEERKAT_WHOLE_BLOB_GENERATION_MESSAGES",
        DEFAULT_GENERATION_MESSAGES,
    );
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let client = TextClient::default();

    // ---------------- Lifetime 1 ----------------
    let service_1 = persistent_service(&paths, &client);
    let handle_1 = MobBuilder::new(
        mob_definition(),
        MobStorage::persistent(&paths.mob_db).expect("mob storage"),
    )
    .with_session_service(service_1.clone())
    .with_default_llm_client(Arc::new(client.clone()))
    .create()
    .await
    .expect("create mob");
    handle_1
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from(LEAD_ID)))
        .await
        .expect("spawn lead");
    for member in LARGE_MEMBER_IDS {
        handle_1
            .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from(member)))
            .await
            .expect("spawn worker");
    }
    let member_count = 1 + LARGE_MEMBER_IDS.len();
    wait_all_active(&handle_1, member_count, "before seeding").await;
    for member in LARGE_MEMBER_IDS {
        run_turn(&handle_1, member, format!("seed turn for {member}")).await;
    }
    let mut session_ids = Vec::new();
    for member in LARGE_MEMBER_IDS.into_iter().chain([LEAD_ID]) {
        session_ids.push(
            handle_1
                .resolve_bridge_session_id(&AgentIdentity::from(member))
                .await
                .expect("member session id"),
        );
    }
    handle_1.shutdown().await.expect("shutdown");
    for session_id in &session_ids {
        service_1
            .discard_live_session(session_id)
            .await
            .expect("discard live session");
    }
    drop(handle_1);
    drop(service_1);

    // Replace each large member's committed document with a deep-history
    // transcript, committed through the store's own WholeBlob snapshot verb.
    let store = meerkat_runtime::SqliteRuntimeStore::new_whole_blob(&paths.runtime_db)
        .expect("reopen WholeBlob runtime store");
    let mut document_bytes = 0u64;
    for session_id in session_ids.iter().take(LARGE_MEMBER_IDS.len()) {
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        let committed = store
            .load_committed_whole_blob_snapshot(&runtime_id)
            .await
            .expect("load committed snapshot")
            .expect("committed snapshot exists");
        let mut session = committed.session().clone();
        synthesize_rewrite_history(&mut session, generations, generation_messages);
        let bytes = session
            .to_persisted_bytes()
            .expect("encode fixture document");
        document_bytes += bytes.len() as u64;
        eprintln!(
            "[whole-blob resume] {session_id}: {} live messages, generation {}, {} document bytes",
            session.messages().len(),
            session.transcript_rewrite_generation().expect("generation"),
            bytes.len()
        );
        store
            .commit_session_snapshot(
                &runtime_id,
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: Arc::new(bytes),
                },
            )
            .await
            .expect("commit deep-history fixture");
    }
    drop(store);

    // ---------------- Lifetime 2: cold resume ----------------
    let service_2 = persistent_service(&paths, &client);
    let resume_start = CostMark::now();
    let resume_clock = Instant::now();
    let handle_2 =
        MobBuilder::for_resume(MobStorage::persistent(&paths.mob_db).expect("reopen mob storage"))
            .with_session_service(service_2.clone())
            .with_default_llm_client(Arc::new(client.clone()))
            .notify_orchestrator_on_resume(false)
            .resume()
            .await
            .expect("mob resume");
    wait_all_active(&handle_2, member_count, "after resume").await;
    let resume = CostMark::now().since(resume_start);
    eprintln!(
        "[whole-blob resume] resume to all-active: {:?}",
        resume_clock.elapsed()
    );
    resume.report("resume", member_count, document_bytes);
    let retained_after_resume = service_2.whole_blob_body_cache_retained_bytes();
    eprintln!(
        "[whole-blob resume] verified-body cache after resume: {retained_after_resume} bytes"
    );

    let first_start = CostMark::now();
    let first_clock = Instant::now();
    for member in LARGE_MEMBER_IDS {
        run_turn(
            &handle_2,
            member,
            format!("first turn after resume for {member}"),
        )
        .await;
    }
    let first = CostMark::now().since(first_start);
    eprintln!(
        "[whole-blob resume] first turns: {:?}",
        first_clock.elapsed()
    );
    first.report("first-turn", member_count, document_bytes);
    let retained_after_first = service_2.whole_blob_body_cache_retained_bytes();
    eprintln!(
        "[whole-blob resume] verified-body cache after first turns: {retained_after_first} bytes"
    );

    // Fill the verified-body cache through a read of each live member (an
    // observation read, outside every measured window). The turns below must
    // evict those bodies as they commit newer authorities, not leave them as
    // dead copies until budget eviction.
    for session_id in session_ids.iter().take(LARGE_MEMBER_IDS.len()) {
        service_2
            .observe_authoritative_session_body(session_id)
            .await
            .expect("observe committed body")
            .expect("committed body present");
    }
    let retained_after_reads = service_2.whole_blob_body_cache_retained_bytes();
    eprintln!(
        "[whole-blob resume] verified-body cache after observation reads: {retained_after_reads} bytes"
    );
    assert!(
        retained_after_reads > 0,
        "the observation reads must populate the cache for this check to mean anything"
    );

    // Steady state: by now every committed head was written by a per-turn
    // runtime boundary the session service never saw, so any consumer that
    // still needs the committed body has to decode it.
    let steady_start = CostMark::now();
    let steady_clock = Instant::now();
    for round in 0..STEADY_TURNS_PER_MEMBER {
        for member in LARGE_MEMBER_IDS {
            run_turn(
                &handle_2,
                member,
                format!("steady turn {round} after resume for {member}"),
            )
            .await;
        }
    }
    let steady = CostMark::now().since(steady_start);
    eprintln!(
        "[whole-blob resume] steady turns ({STEADY_TURNS_PER_MEMBER} per member): {:?}",
        steady_clock.elapsed()
    );
    steady.report("steady-turns", member_count, document_bytes);
    let retained_after_steady = service_2.whole_blob_body_cache_retained_bytes();
    eprintln!(
        "[whole-blob resume] verified-body cache after steady turns: {retained_after_steady} bytes"
    );

    handle_2.shutdown().await.expect("final shutdown");

    let sessions = member_count as u64;
    assert!(
        resume.decodes <= MAX_RESUME_DECODES_PER_SESSION * sessions,
        "cold resume decoded {} committed documents for {sessions} sessions; every consumer \
         after the store-owned recovery must reuse its verified body",
        resume.decodes
    );
    assert!(
        resume.graph_validations <= MAX_RESUME_DECODES_PER_SESSION * LARGE_MEMBER_IDS.len() as u64,
        "cold resume validated {} rewrite graphs for {} rewritten sessions; validation must \
         run once per decode, and decodes must not repeat per consumer",
        resume.graph_validations,
        LARGE_MEMBER_IDS.len()
    );
    let digest_budget = (document_bytes as f64 * MAX_RESUME_DIGEST_MULTIPLE) as u64;
    assert!(
        resume.digest_bytes <= digest_budget,
        "cold resume hashed {} content-digest bytes for {document_bytes} committed document \
         bytes (budget {digest_budget})",
        resume.digest_bytes
    );
    assert_eq!(
        first.decodes, 0,
        "the first turn after a resume must reuse the committed body the resume verified"
    );
    assert_eq!(
        first.graph_validations, 0,
        "the first turn after a resume must not re-validate an unchanged rewrite graph"
    );
    // Retention is scoped to the resume window: once every actor has
    // materialized and the create-time save advanced each authority, no
    // verified body may outlive it and duplicate a live transcript.
    assert_eq!(
        (
            retained_after_resume,
            retained_after_first,
            retained_after_steady
        ),
        (0, 0, 0),
        "the verified WholeBlob body cache must not retain bodies past the resume window"
    );
    assert_eq!(
        steady.decodes, 0,
        "a steady-state turn must classify live/committed authority from bounded facts, \
         not decode the committed document"
    );
    assert_eq!(steady.graph_validations, 0);
    let steady_digest_budget = (document_bytes as f64
        * MAX_FIRST_TURN_DIGEST_MULTIPLE
        * STEADY_TURNS_PER_MEMBER as f64) as u64;
    assert!(
        steady.digest_bytes <= steady_digest_budget,
        "{STEADY_TURNS_PER_MEMBER} steady turns per member hashed {} content-digest bytes for \
         {document_bytes} committed document bytes (budget {steady_digest_budget})",
        steady.digest_bytes
    );
    let first_turn_digest_budget = (document_bytes as f64 * MAX_FIRST_TURN_DIGEST_MULTIPLE) as u64;
    assert!(
        first.digest_bytes <= first_turn_digest_budget,
        "the first turn after a resume hashed {} content-digest bytes for {document_bytes} \
         committed document bytes (budget {first_turn_digest_budget})",
        first.digest_bytes
    );
}
