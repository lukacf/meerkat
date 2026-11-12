//! A member session keeps the instruction sections it was built with.
//!
//! A session's durable build state records the instruction sections its
//! build was given (`SessionBuildState::additional_instructions`). A mob
//! member's first build records them. Every later build of the member (a
//! restart's restore, a wake, a fork child's seating) resumes its session
//! and names no sections, because a resume authors nothing: it must keep the
//! recorded sections, not erase them.
//! `MobSessionService::recorded_build_instructions` reads them from
//! committed state without waking the member.
//!
//! The rows run a persistent mob on the production realm shape (SQLite
//! session rows and HeadCanonical runtime authority in one database) with
//! the real agent factory and a scripted provider, across cold restarts.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_core::types::{HandlingMode, SessionId};
use meerkat_mob::{
    AgentIdentity, DormantSeatOutcome, MemberTurnOptions, MobBuilder, MobDefinition, MobHandle,
    MobId, MobMemberStatus, MobRuntimeMode, MobStorage, Profile, ProfileBinding, ProfileName,
    SpawnMemberSpec, ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, SqliteSessionStore, StoreAdapter};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{Duration, Instant};

const SECTION_ONE: &str = "Keep answers to one line.";
const SECTION_TWO: &str = "Name the desk you answer for.";

/// Scripted provider: every request ends its turn with a short answer.
#[derive(Clone, Default)]
struct ShortAnswerClient {
    requests: Arc<AtomicUsize>,
}

impl ShortAnswerClient {
    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for ShortAnswerClient {
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
                delta: "noted".to_string(),
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
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
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

fn worker_profile() -> Profile {
    Profile {
        model_fallback: None,
        model: "gpt-5.5".to_string(),
        provider: None,
        self_hosted_server_id: None,
        auth_binding: None,
        image_generation_provider: None,
        auto_compact_threshold: None,
        resume_overrides: Vec::new(),
        skills: vec![],
        tools: ToolConfig {
            comms: true,
            ..Default::default()
        },
        peer_description: "Answers for one desk".to_string(),
        external_addressable: true,
        backend: None,
        runtime_mode: MobRuntimeMode::TurnDriven,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
        durable: false,
    }
}

fn mob_definition() -> MobDefinition {
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "build-sections-{}",
        meerkat_core::time_compat::new_uuid_v7()
    )));
    definition.profiles = BTreeMap::from([(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(worker_profile())),
    )]);
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
            fs::create_dir_all(dir).expect("create the mob's roots");
        }
        paths
    }
}

type Service = Arc<PersistentSessionService<FactoryAgentBuilder>>;

/// SQLite session rows and HeadCanonical runtime authority in one database.
fn persistent_service(paths: &Paths, client: &ShortAnswerClient) -> Service {
    let factory = AgentFactory::new(paths.root.join("runtime-root").join("factory-store"))
        .user_config_root(paths.root.join("user-config"))
        .runtime_root(paths.root.join("runtime-root"))
        .project_root(paths.root.join("project-root"))
        .context_root(paths.root.join("context-root"))
        .builtins(true)
        .comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(client.clone()));
    let store =
        Arc::new(SqliteSessionStore::open(&paths.realm_db_path).expect("SQLite session store"));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));
    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&paths.realm_db_path)
            .expect("HeadCanonical runtime store"),
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

/// One process lifetime of the mob over the same files.
struct Life {
    service: Service,
    handle: MobHandle,
}

impl Life {
    /// The first lifetime: a new mob.
    async fn create(paths: &Paths, client: &ShortAnswerClient) -> Self {
        let service = persistent_service(paths, client);
        let handle = MobBuilder::new(
            mob_definition(),
            MobStorage::persistent(&paths.mob_db_path).expect("persistent mob storage"),
        )
        .with_session_service(service.clone())
        .with_default_llm_client(Arc::new(client.clone()))
        .create()
        .await
        .expect("create the mob");
        Self { service, handle }
    }

    /// A later lifetime: the mob resumed from its files under the default
    /// restore policy, which leaves a member with no work unmaterialized.
    async fn resume(paths: &Paths, client: &ShortAnswerClient) -> Self {
        let service = persistent_service(paths, client);
        let handle = MobBuilder::for_resume(
            MobStorage::persistent(&paths.mob_db_path).expect("reopen mob storage"),
        )
        .with_session_service(service.clone())
        .with_default_llm_client(Arc::new(client.clone()))
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("resume the mob");
        Self { service, handle }
    }

    /// Spawn `member` as a worker built with `sections`.
    async fn spawn(&self, member: &str, sections: Option<Vec<String>>) -> SessionId {
        let mut spec = SpawnMemberSpec::new("worker", AgentIdentity::from(member));
        spec.additional_instructions = sections;
        self.handle
            .spawn_spec(spec)
            .await
            .expect("spawn the member");
        self.wait_active(member).await;
        self.session_of(member).await
    }

    async fn session_of(&self, member: &str) -> SessionId {
        self.handle
            .resolve_bridge_session_id(&AgentIdentity::from(member))
            .await
            .expect("the member's session")
    }

    /// Wait on the mob actor's own state publications until `member` reads
    /// active; the deadline only bounds a broken run.
    async fn wait_active(&self, member: &str) {
        let identity = AgentIdentity::from(member);
        let deadline = Instant::now() + Duration::from_mins(2);
        let mut changes = self.handle.machine_state_changes();
        loop {
            let members = self.handle.list_members().await;
            if members.iter().any(|entry| {
                entry.agent_identity == identity && entry.status == MobMemberStatus::Active
            }) {
                return;
            }
            tokio::select! {
                changed = changes.changed() => changed.unwrap_or_else(|_| {
                    panic!("the mob actor stopped before {member} was active: {members:?}")
                }),
                () = tokio::time::sleep_until(deadline) => {
                    panic!("timed out waiting for {member} to be active: {members:?}")
                }
            }
        }
    }

    /// One committed turn of `member`.
    async fn turn(&self, member: &str, prompt: &str) {
        self.handle
            .member(&AgentIdentity::from(member))
            .await
            .expect("member handle")
            .start_turn(
                prompt.to_string(),
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

    /// The instruction sections `session`'s committed build state records.
    async fn recorded_sections(&self, session: &SessionId) -> Option<Vec<String>> {
        self.service
            .load_authoritative_session(session)
            .await
            .expect("load the committed session")
            .expect("the session is committed")
            .build_state()
            .expect("a built session records its build state")
            .additional_instructions
    }

    /// End this lifetime: stop the mob and drop every live session, so the
    /// next lifetime finds only committed state.
    async fn end(self, sessions: &[&SessionId]) {
        self.handle.shutdown().await.expect("shut the mob down");
        for session in sessions {
            self.service
                .discard_live_session(session)
                .await
                .expect("discard the live session");
        }
    }
}

fn sections() -> Vec<String> {
    vec![SECTION_ONE.to_string(), SECTION_TWO.to_string()]
}

/// The reported shape: a member built with sections, a restart, one turn.
/// The turn's wake resumes the member's session; the session still records
/// the sections it was built with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_members_build_sections_survive_a_restart_and_one_turn() {
    let _window = DIGEST_WINDOW.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let client = ShortAnswerClient::default();

    let first = Life::create(&paths, &client).await;
    let session = first.spawn("desk-north", Some(sections())).await;
    first.turn("desk-north", "first turn").await;
    assert_eq!(
        first.recorded_sections(&session).await,
        Some(sections()),
        "the first build records the sections it was given"
    );
    first.end(&[&session]).await;

    let second = Life::resume(&paths, &client).await;
    second.wait_active("desk-north").await;
    assert_eq!(
        second.session_of("desk-north").await,
        session,
        "the restart keeps the member on its session"
    );
    assert_eq!(
        second.recorded_sections(&session).await,
        Some(sections()),
        "a restart alone leaves the recorded sections in place"
    );
    let requests_before = client.requests();
    second.turn("desk-north", "turn after the restart").await;
    assert!(
        client.requests() > requests_before,
        "instrument honesty: the turn after the restart reached the provider"
    );
    assert_eq!(
        second.recorded_sections(&session).await,
        Some(sections()),
        "the resumed build of the turn after a restart keeps the recorded sections"
    );
    second.end(&[&session]).await;
}

/// The sections survive every resume, not only the first one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_members_build_sections_survive_two_resumes() {
    let _window = DIGEST_WINDOW.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let client = ShortAnswerClient::default();

    let first = Life::create(&paths, &client).await;
    let session = first.spawn("desk-north", Some(sections())).await;
    first.turn("desk-north", "first turn").await;
    first.end(&[&session]).await;

    for lifetime in ["second", "third"] {
        let life = Life::resume(&paths, &client).await;
        life.wait_active("desk-north").await;
        life.turn("desk-north", &format!("turn of the {lifetime} lifetime"))
            .await;
        assert_eq!(
            life.recorded_sections(&session).await,
            Some(sections()),
            "the {lifetime} lifetime's resumed build keeps the recorded sections"
        );
        life.end(&[&session]).await;
    }
}

/// A member built with no sections records that it was built with none (an
/// empty list), which a resume keeps. An absent record means the sections
/// are not known: a session an earlier release built, or one whose record an
/// earlier release's resume erased.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_built_with_no_sections_records_an_empty_list_across_a_resume() {
    let _window = DIGEST_WINDOW.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let client = ShortAnswerClient::default();

    let first = Life::create(&paths, &client).await;
    let session = first.spawn("desk-plain", None).await;
    first.turn("desk-plain", "first turn").await;
    assert_eq!(
        first.recorded_sections(&session).await,
        Some(Vec::new()),
        "a first build that names no sections records an empty list"
    );
    first.end(&[&session]).await;

    let second = Life::resume(&paths, &client).await;
    second.wait_active("desk-plain").await;
    second.turn("desk-plain", "turn after the restart").await;
    assert_eq!(
        second.recorded_sections(&session).await,
        Some(Vec::new()),
        "the resumed build keeps the recorded empty list"
    );
    second.end(&[&session]).await;
}

/// A fork child continues its source's transcript, whose prompt rows were
/// built with the source's sections, and a fork names no sections of its
/// own. The child's committed build state starts as the source's, and the
/// child's seating (a resume of its committed session) keeps it: the child
/// records its source's sections.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_childs_recorded_sections_are_its_sources() {
    let _window = DIGEST_WINDOW.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let client = ShortAnswerClient::default();

    let life = Life::create(&paths, &client).await;
    let source = life.spawn("desk-north", Some(sections())).await;
    life.turn("desk-north", "first turn").await;

    let fork = life
        .handle
        .fork_member(
            &AgentIdentity::from("desk-north"),
            SpawnMemberSpec::new("worker", AgentIdentity::from("desk-north-branch")),
            None,
        )
        .await
        .expect("fork the member");
    assert_eq!(fork.source_session_id, source);
    assert_ne!(
        fork.session_id, source,
        "the child has a session of its own"
    );
    life.wait_active("desk-north-branch").await;
    assert_eq!(
        life.recorded_sections(&fork.session_id).await,
        Some(sections()),
        "the fork child records its source's sections after its seating build"
    );
    life.turn("desk-north-branch", "the branch's first turn")
        .await;
    assert_eq!(
        life.recorded_sections(&fork.session_id).await,
        Some(sections()),
        "the fork child still records them after its own turn"
    );
    assert_eq!(
        life.recorded_sections(&source).await,
        Some(sections()),
        "the fork leaves the source's record alone"
    );
    life.end(&[&source, &fork.session_id]).await;
}

/// Loading a session with its transcript moves a process-wide digest
/// counter, which one row measures. Every row of this binary holds this
/// window for its whole run, so no other row moves the counter meanwhile
/// (nextest runs each row in its own process; plain `cargo test` shares
/// one).
static DIGEST_WINDOW: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The measured member's first turn carries this much text, so its
/// transcript is at least this large: a read that loads the transcript
/// digests at least this many bytes, and one that reads the committed
/// metadata alone digests a small fraction of it.
const TRANSCRIPT_FILLER_BYTES: usize = 256 * 1024;

async fn live_actor_registered(service: &Service, session: &SessionId) -> bool {
    meerkat_mob::MobSessionService::live_session_actor_registered(service.as_ref(), session)
        .await
        .expect("read the live actor registry")
}

/// The session service's read of a member session's recorded sections.
async fn read_recorded(service: &Service, session: &SessionId) -> Option<Vec<String>> {
    meerkat_mob::MobSessionService::recorded_build_instructions(service.as_ref(), session)
        .await
        .expect("read the session's recorded sections")
}

/// `MobSessionService::recorded_build_instructions` answers what a member
/// session's committed build state records, for a live member and for a
/// member a restart left unmaterialized. The read wakes nothing: the member
/// stays unmaterialized, no session actor exists, no provider request is
/// made, and the transcript is not loaded (the content digest counter, which
/// a load of the session's rows moves by at least their size, moves by a
/// small fraction of it: the committed head's metadata).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_recorded_sections_read_wakes_no_member_and_loads_no_transcript() {
    let _window = DIGEST_WINDOW.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let client = ShortAnswerClient::default();
    let member = AgentIdentity::from("desk-north");

    let first = Life::create(&paths, &client).await;
    let session = first.spawn("desk-north", Some(sections())).await;
    first
        .turn(
            "desk-north",
            &format!(
                "first turn {}",
                "filler ".repeat(TRANSCRIPT_FILLER_BYTES / 7)
            ),
        )
        .await;
    assert_eq!(
        read_recorded(&first.service, &session).await,
        Some(sections()),
        "a live member's session answers what it records"
    );
    first.end(&[&session]).await;

    let second = Life::resume(&paths, &client).await;
    second.wait_active("desk-north").await;
    assert!(
        second.handle.member_unmaterialized(&member),
        "the restart leaves a member with no work unmaterialized"
    );
    let requests_before = client.requests();
    let digest_before = meerkat_core::global_session_content_digest_bytes();
    assert_eq!(
        read_recorded(&second.service, &session).await,
        Some(sections()),
        "an unmaterialized member's session answers from its committed state"
    );
    let digest_after_read = meerkat_core::global_session_content_digest_bytes();
    let read_digest = digest_after_read - digest_before;
    assert!(
        read_digest < (TRANSCRIPT_FILLER_BYTES / 8) as u64,
        "the read does not load the transcript: it digested {read_digest} bytes of a session \
         whose transcript holds at least {TRANSCRIPT_FILLER_BYTES}"
    );
    assert!(
        second.handle.member_unmaterialized(&member),
        "the read leaves the member unmaterialized"
    );
    assert!(
        !live_actor_registered(&second.service, &session).await,
        "the read builds no session actor"
    );
    assert_eq!(
        client.requests(),
        requests_before,
        "the read makes no provider request"
    );
    second
        .service
        .load_authoritative_session(&session)
        .await
        .expect("load the committed session")
        .expect("the session is committed");
    let load_digest = meerkat_core::global_session_content_digest_bytes() - digest_after_read;
    assert!(
        load_digest >= TRANSCRIPT_FILLER_BYTES as u64,
        "instrument honesty: loading the session with its transcript digests its rows \
         ({load_digest} bytes for at least {TRANSCRIPT_FILLER_BYTES})"
    );
    eprintln!(
        "[build sections] the read digested {read_digest} bytes; a load of the session \
         digested {load_digest}"
    );

    second.turn("desk-north", "turn after the restart").await;
    assert_eq!(
        read_recorded(&second.service, &session).await,
        Some(sections()),
        "the member's resumed build keeps what the read answers"
    );
    second.end(&[&session]).await;
}

/// A seat no build has created yet has no session, so the read answers none
/// for the session it names, and nothing is built for the read: the seat's
/// sections are still its spec's. Once the member's first wake has built the
/// session, the read answers the sections the seat carried to that build
/// (here no spawn customizer appends any). A seat that names no sections
/// then answers an empty list.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unborn_seats_session_answers_none_then_the_sections_its_seat_carried() {
    let _window = DIGEST_WINDOW.lock().await;
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = Paths::new(temp.path());
    let client = ShortAnswerClient::default();
    let carrying = AgentIdentity::from("desk-east");
    let plain = AgentIdentity::from("desk-west");

    let life = Life::create(&paths, &client).await;
    let mut carrying_spec = SpawnMemberSpec::new("worker", carrying.clone());
    carrying_spec.additional_instructions = Some(sections());
    let outcomes = life
        .handle
        .seat_dormant_members(vec![
            carrying_spec,
            SpawnMemberSpec::new("worker", plain.clone()),
        ])
        .await
        .expect("the seat batch answers");
    let seated = outcomes
        .iter()
        .map(|outcome| match outcome {
            DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
            other => panic!("the spec is seated: {other:?}"),
        })
        .collect::<Vec<_>>();
    let expected = [(&carrying, sections()), (&plain, Vec::new())];

    for ((member, _), session) in expected.iter().zip(&seated) {
        assert_eq!(
            read_recorded(&life.service, session).await,
            None,
            "a session no build has created has no record"
        );
        assert!(
            life.handle.session_unborn(session) && life.handle.member_unmaterialized(member),
            "the read builds nothing: the seat's session stays unborn"
        );
    }
    assert_eq!(client.requests(), 0, "no provider request was made");

    for ((member, sections), session) in expected.iter().zip(&seated) {
        life.handle
            .ensure_member_live(member)
            .await
            .expect("the member's first wake builds its session");
        assert!(
            !life.handle.session_unborn(session),
            "the first wake created the session"
        );
        assert_eq!(
            read_recorded(&life.service, session).await,
            Some(sections.clone()),
            "the first build records the sections the seat carried"
        );
    }
    life.end(&[&seated[0], &seated[1]]).await;
}
