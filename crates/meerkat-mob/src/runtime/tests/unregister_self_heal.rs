//! A runtime unregister cut by process exit after its Draining progress is
//! durable is concluded at the session's next registration.
//!
//! A Shutdown reports a member whose unregister outlives its deadline as
//! `UnregisterPending` and leaves the unregister running. When the host
//! process exits before it ends, the session's durable lifecycle keeps the
//! unregister's Draining progress. The next registration of that session
//! recovers it as cold-recovered Draining, concludes the teardown and only
//! then registers the session fresh.
//!
//! The first host is a child process (this test binary again). Over SQLite
//! stores with cross-process hosting claims it parks every member's
//! unregister finalization, the store transaction that persists the final
//! unregister transition, and shuts the mob down with a short deadline. Once
//! every unregister waits at the park, the test kills the child: the exit
//! cuts each one with its Draining progress durable and its final transition
//! not persisted. The test then resumes the mob over the same files and
//! fresh services, as the next boot does.

use super::*;
use crate::runtime::{MemberShutdownOutcome, ShutdownOptions};
use meerkat_runtime::store::MachineLifecycleObservation;

const PHASE_ENV: &str = "MEERKAT_CUT_UNREGISTER_PHASE";
const ROOT_ENV: &str = "MEERKAT_CUT_UNREGISTER_ROOT";
const CHILD_TEST: &str =
    "runtime::tests::actor_isolation::unregister_self_heal::cut_unregister_host_child";
const ANSWER: &str = "self-heal-answer";

/// Bound for one step (the child's spawns and shutdown, the resume, a turn);
/// it only turns a hang into a failure.
const STEP: Duration = Duration::from_secs(150);

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

/// A model that answers every request at once.
#[derive(Clone)]
pub(super) struct AnswerClient;

#[async_trait]
impl meerkat_client::LlmClient for AnswerClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        Box::pin(async_stream::try_stream! {
            yield meerkat_client::LlmEvent::TextDelta {
                delta: ANSWER.to_string(),
                meta: None,
            };
            yield meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    Usage::default(),
                ),
            };
            yield meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            };
        })
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// The realm's session service over the root's SQLite files, its runtime
/// store hosting sessions with OS-locked claims beside the database.
pub(super) async fn open_service(
    root: &std::path::Path,
) -> (Arc<Service>, Arc<FaultInjectingRuntimeStore>) {
    let factory = meerkat::AgentFactory::new(root.join("factory-store"))
        .user_config_root(root.join("config"))
        .runtime_root(root.join("runtime"))
        .project_root(root.join("project"))
        .context_root(root.join("context"))
        .builtins(false)
        .comms(false);
    let mut config = meerkat::Config::default();
    config.agent.model = "gpt-5.5".to_string();
    config.compaction.auto_compact_threshold = 1_000_000;
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(Arc::new(AnswerClient));
    let db = root.join("realm.db");
    let session_store =
        Arc::new(meerkat_store::SqliteSessionStore::open(&db).expect("session store"));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    let sqlite = meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&db)
        .expect("head-canonical runtime store")
        .with_hosting_paths(meerkat_runtime::HostingPaths {
            hosting_lock_dir: root.join("hosting"),
            cold_delivery_lock: root.join("delivery").join("cold-delivery.lock"),
            database: None,
        });
    assert!(
        matches!(
            sqlite.hosting_capability(),
            meerkat_runtime::HostingCapability::OsLock(_)
        ),
        "the runtime store hosts sessions with cross-process claims"
    );
    let store = FaultInjectingRuntimeStore::wrapping(Arc::new(sqlite));
    let runtime_store: Arc<dyn RuntimeStore> = store.clone();
    let service = Arc::new(Service::new(
        builder,
        128,
        session_store,
        runtime_store,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    (service, store)
}

pub(super) fn mob_storage(root: &std::path::Path) -> MobStorage {
    MobStorage::persistent(root.join("mob.db")).expect("persistent mob storage")
}

pub(super) fn definition(mob_id: &str) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: "gpt-5.5".into(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..ToolConfig::default()
            },
            peer_description: "A worker".into(),
            external_addressable: false,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(mob_id.to_string()));
    definition.profiles = profiles;
    definition
}

fn members(count: usize) -> Vec<AgentIdentity> {
    (0..count)
        .map(|index| AgentIdentity::from(format!("worker-{index:02}")))
        .collect()
}

/// Whether the session's durable lifecycle still carries unregister
/// progress (Draining).
async fn draining_recorded(store: &FaultInjectingRuntimeStore, session: &SessionId) -> bool {
    match store
        .observe_machine_lifecycle(&LogicalRuntimeId::for_session(session))
        .await
        .expect("read the durable lifecycle")
    {
        MachineLifecycleObservation::Decoded { record, .. } => {
            record.unregister_progress().is_some()
        }
        other => panic!("{session}: the durable lifecycle is not a decoded record: {other:?}"),
    }
}

/// The first host. It runs only when the parent re-executes this binary;
/// as an ordinary test it does nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cut_unregister_host_child() {
    let (Some(root), Some(_)) = (std::env::var_os(ROOT_ENV), std::env::var_os(PHASE_ENV)) else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mob_id = std::fs::read_to_string(root.join("mob-id")).expect("mob id");
    let count: usize = std::fs::read_to_string(root.join("members"))
        .expect("member count")
        .trim()
        .parse()
        .expect("member count is a number");
    let (service, store) = open_service(&root).await;
    let handle = MobBuilder::new(definition(mob_id.trim()), mob_storage(&root))
        .with_session_service(service)
        .with_default_llm_client(Arc::new(AnswerClient))
        .create()
        .await
        .expect("create the persistent mob");
    let identities = members(count);
    let spawned = handle
        .spawn_many(
            identities
                .iter()
                .map(|identity| SpawnMemberSpec::new("worker", identity.clone()))
                .collect(),
        )
        .await
        .expect("spawn the members");
    for (identity, result) in identities.iter().zip(&spawned) {
        assert!(result.is_ok(), "{identity}: {result:?}");
    }
    let mut sessions = Vec::with_capacity(count);
    for identity in &identities {
        let session = handle
            .resolve_bridge_session_id(identity)
            .await
            .expect("member session");
        store.park_unregister_finalizations(&session);
        sessions.push(session);
    }
    std::fs::write(
        root.join("sessions"),
        sessions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("record the member sessions");

    // The deadline only has to admit every member's unregister: none can
    // finish while its finalization is parked.
    let report = handle
        .shutdown_with_report(
            ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(5)),
        )
        .await
        .expect("the shutdown answers");
    for identity in &identities {
        assert!(
            matches!(
                report.members.get(identity),
                Some(MemberShutdownOutcome::UnregisterPending { stage, .. })
                    if stage == "shutdown_runtime_unregister"
            ),
            "{identity}: the admitted unregister outlives the deadline: {report:?}"
        );
    }
    // Every unregister reaches its parked finalization: its Draining
    // progress is durable and its final transition is not.
    tokio::time::timeout(STEP, async {
        while sessions
            .iter()
            .any(|session| store.parked_unregister_finalization_arrivals(session) == 0)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("every unregister reaches its parked finalization");
    std::fs::write(root.join("parked"), "parked\n").expect("write the marker");
    std::future::pending::<()>().await;
}

/// Run the child host with `count` members until every unregister waits at
/// its parked finalization, kill it there, and resume the mob over the same
/// files. Returns the resumed mob with its store, sessions and members.
// The host is this test binary re-executed: a test-only process launch.
#[allow(clippy::disallowed_methods)]
async fn cut_then_resume(
    count: usize,
) -> (
    tempfile::TempDir,
    MobHandle,
    Arc<FaultInjectingRuntimeStore>,
    Vec<SessionId>,
    Vec<AgentIdentity>,
) {
    let root = tempfile::Builder::new()
        .prefix("meerkat-cut-unregister-")
        .tempdir()
        .expect("fixture storage in the system temp dir");
    let root_path = root.path().canonicalize().expect("absolute fixture root");
    for directory in [
        "config", "runtime", "project", "context", "hosting", "delivery",
    ] {
        std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
    }
    let mob_id = format!("cut-unregister-{}", uuid::Uuid::new_v4().simple());
    std::fs::write(root_path.join("mob-id"), &mob_id).expect("mob id");
    std::fs::write(root_path.join("members"), count.to_string()).expect("member count");

    let log_path = root_path.join("child.log");
    let log = std::fs::File::create(&log_path).expect("child log");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(PHASE_ENV, "host")
        .env(ROOT_ENV, &root_path)
        .stdout(log.try_clone().expect("child log"))
        .stderr(log)
        .spawn()
        .expect("spawn the host");
    let marker = root_path.join("parked");
    let started = Instant::now();
    while !marker.exists() {
        let exited = child.try_wait().expect("poll the host");
        assert!(
            exited.is_none() && started.elapsed() < STEP,
            "the host never parked its unregisters ({exited:?}): {}",
            std::fs::read_to_string(&log_path).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The exit cuts every unregister at its parked finalization.
    child.kill().expect("kill the host");
    child.wait().expect("reap the host");

    let sessions = std::fs::read_to_string(root_path.join("sessions"))
        .expect("member sessions")
        .lines()
        .map(|line| SessionId::parse(line).expect("session id"))
        .collect::<Vec<_>>();
    assert_eq!(sessions.len(), count);
    let (service, store) = open_service(&root_path).await;
    for session in &sessions {
        assert!(
            draining_recorded(&store, session).await,
            "{session}: the cut unregister left its Draining progress durable"
        );
    }

    let resume = MobBuilder::for_resume(mob_storage(&root_path))
        .with_session_service(service)
        .with_default_llm_client(Arc::new(AnswerClient))
        .notify_orchestrator_on_resume(false)
        .resume();
    let handle = tokio::time::timeout(STEP, resume)
        .await
        .expect("the resume answers")
        .expect("the first resume after the cut succeeds");
    (root, handle, store, sessions, members(count))
}

/// Every member came back on the first resume, in its own session, and no
/// session keeps the Draining progress of its cut unregister.
async fn assert_every_member_back(
    handle: &MobHandle,
    store: &FaultInjectingRuntimeStore,
    sessions: &[SessionId],
    identities: &[AgentIdentity],
) {
    assert_eq!(
        handle.status().await.expect("mob status"),
        MobState::Running
    );
    let failures = handle.restore_diagnostics.read().await.clone();
    assert!(
        failures.is_empty(),
        "no member restore failed: {failures:?}"
    );
    let listed = handle.list_members().await;
    for (identity, session) in identities.iter().zip(sessions) {
        let entry = listed
            .iter()
            .find(|entry| &entry.agent_identity == identity)
            .unwrap_or_else(|| panic!("{identity} is listed: {listed:?}"));
        assert_eq!(
            entry.status,
            crate::runtime::handle::MobMemberStatus::Active,
            "{entry:?}"
        );
        assert_eq!(
            handle.resolve_bridge_session_id(identity).await.as_ref(),
            Some(session),
            "{identity} is back in its own session"
        );
        assert!(
            !draining_recorded(store, session).await,
            "{session}: the registration concluded the cut unregister"
        );
    }
}

async fn assert_member_turn(handle: &MobHandle, identity: &AgentIdentity) {
    let spec = BoundedResultSpec::new("self-heal", 256).expect("bounded result spec");
    let turn = handle
        .start_work_for_identity_bounded(
            identity.clone(),
            WorkSpec::new(
                ContentInput::Text("are you back?".to_string()),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("the member takes a turn");
    let result = tokio::time::timeout(STEP, turn.wait_bounded(spec))
        .await
        .expect("the turn completes")
        .expect("the turn succeeds");
    assert_eq!(result.result().result().text(), ANSWER);
}

/// One member whose unregister the exit cut comes back on the next boot and
/// takes a turn: its registration recovered the Draining record, concluded
/// the cut unregister and registered the session fresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unregister_cut_by_process_exit_is_concluded_at_the_next_registration() {
    let (_root, handle, store, sessions, identities) = cut_then_resume(1).await;
    assert_every_member_back(&handle, &store, &sessions, &identities).await;
    assert_member_turn(&handle, &identities[0]).await;
    handle.shutdown().await.expect("shut the resumed mob down");
}

/// The first boot after a shutdown whose exit cut 64 unregisters (a fleet's
/// tail) brings every member back: the cold teardowns at restore do not fail
/// the start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_resume_after_many_cut_unregisters_brings_every_member_back() {
    let (_root, handle, store, sessions, identities) = cut_then_resume(64).await;
    assert_every_member_back(&handle, &store, &sessions, &identities).await;
    assert_member_turn(&handle, &identities[identities.len() - 1]).await;
    handle.shutdown().await.expect("shut the resumed mob down");
}
