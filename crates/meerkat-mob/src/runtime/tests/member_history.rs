#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! Member binding history over real persistent stores.
//!
//! Every test runs real turn-driven members over a real factory, a real
//! SQLite session store and a real head-canonical SQLite runtime store, with
//! persistent mob storage. The handle reads fold the same durable journal a
//! restart rebuilds from; the store-level reader then reads the stopped mob's
//! file and must answer the same.

use super::*;
use crate::runtime::{MemberBindingHistoryReadError, MemberBindingState, MemberIdentityManagement};
use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};

const WAIT: Duration = Duration::from_secs(30);

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

/// Answers every turn with one fixed text block.
#[derive(Clone)]
struct ScriptedClient;

#[async_trait]
impl meerkat_client::LlmClient for ScriptedClient {
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
                delta: "history-answer".to_string(),
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

/// The realm's session service over the fixture's SQLite files.
fn open_service(root: &std::path::Path) -> Arc<Service> {
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
    builder.default_llm_client = Some(Arc::new(ScriptedClient));
    let db = root.join("realm.db");
    let session_store =
        Arc::new(meerkat_store::SqliteSessionStore::open(&db).expect("session store"));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&db)
            .expect("head-canonical runtime store"),
    );
    Arc::new(Service::new(
        builder,
        16,
        session_store,
        runtime_store,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ))
}

fn turn_driven_definition(label: &str) -> MobDefinition {
    let mut definition = with_unique_mob_id(sample_definition(), label);
    let lead = definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(ProfileBinding::as_inline_mut)
        .expect("inline lead");
    lead.model = "gpt-5.5".to_string();
    lead.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    definition
}

/// Unique per test: member turn gates and comms routes are process-wide.
fn unique_identity(label: &str) -> AgentIdentity {
    AgentIdentity::from(format!("{label}-{}", Uuid::new_v4().simple()))
}

fn durable_spec(identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("lead", identity.clone());
    spec.continuity_intent = SpawnContinuityIntent::DurableIdentity {
        continuity_key: identity.to_string(),
    };
    spec
}

/// A host's wake: resume the identity onto the session its last binding
/// names, or spawn it fresh when it has none.
async fn wake(handle: &MobHandle, identity: &AgentIdentity) -> SessionId {
    let spec = match handle
        .last_member_binding(identity)
        .await
        .expect("read the history")
    {
        Some(binding) => durable_spec(identity).with_resume_bridge_session_id(
            binding
                .session_id()
                .cloned()
                .expect("a session-backed binding"),
        ),
        None => durable_spec(identity),
    };
    handle.ensure_member(spec).await.expect("wake the identity");
    handle
        .resolve_bridge_session_id(identity)
        .await
        .expect("a session-backed member")
}

/// One running mob over persistent storage in its own directory.
struct Fixture {
    root: tempfile::TempDir,
    root_path: std::path::PathBuf,
    handle: MobHandle,
    service: Arc<Service>,
}

impl Fixture {
    async fn new(label: &str) -> Self {
        Self::create(label, None).await
    }

    async fn create(label: &str, base_prompt: Option<&str>) -> Self {
        Self::create_with(turn_driven_definition(label), base_prompt).await
    }

    async fn create_with(definition: MobDefinition, base_prompt: Option<&str>) -> Self {
        let root = tempfile::Builder::new()
            .prefix("meerkat-member-history-")
            .tempdir()
            .expect("fixture storage in the system temp dir");
        let root_path = root.path().canonicalize().expect("absolute fixture root");
        for directory in ["config", "runtime", "project", "context"] {
            std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
        }
        let service = open_service(&root_path);
        let storage =
            MobStorage::persistent(root_path.join("mob.db")).expect("persistent mob storage");
        let mut builder = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .with_default_llm_client(Arc::new(ScriptedClient));
        if let Some(prompt) = base_prompt {
            builder = builder.with_spawn_base_prompt_source(Arc::new(
                crate::StaticSpawnBasePromptSource(prompt.to_string()),
            ));
        }
        let handle = builder.create().await.expect("create a persistent mob");
        Self {
            root,
            root_path,
            handle,
            service,
        }
    }

    fn mob_db(&self) -> std::path::PathBuf {
        self.root_path.join("mob.db")
    }

    /// Run one turn on `identity` and wait for its terminal.
    async fn run_turn(&self, identity: &AgentIdentity, text: &str) {
        let spec = BoundedResultSpec::new("member-history", 4096).expect("bounded result spec");
        let turn = self
            .handle
            .start_work_for_identity_bounded(
                identity.clone(),
                WorkSpec::new(text, WorkOrigin::Internal),
                HandlingMode::Queue,
                spec.clone(),
            )
            .await
            .expect("admit the member's turn");
        tokio::time::timeout(WAIT, turn.wait_bounded(spec))
            .await
            .expect("the turn finishes")
            .expect("the turn succeeds");
    }

    async fn transcript(&self, session_id: &SessionId) -> String {
        let page = SessionServiceHistoryExt::read_history(
            self.service.as_ref(),
            session_id,
            SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect("read the transcript");
        serde_json::to_string(&page.messages).expect("transcript json")
    }

    /// Stop the mob and release its process-wide routes; the files stay.
    async fn stop(self) -> tempfile::TempDir {
        let mob_id = self.handle.mob_id().clone();
        tokio::time::timeout(WAIT, self.handle.shutdown())
            .await
            .expect("mob shutdown finishes")
            .expect("mob shutdown");
        drop(self.handle);
        await_supervisor_route_release(&mob_id).await;
        self.root
    }
}

// A retired member keeps its last binding, which names its session;
// resuming the identity onto that session restores its transcript; the
// session then reads as the latest generation that held it; the stopped
// mob's store answers the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retired_member_keeps_its_last_binding_and_resuming_it_restores_the_transcript() {
    let fixture = Fixture::new("member-history-retire").await;
    let handle = &fixture.handle;
    let member = unique_identity("retired-member");
    handle
        .spawn_spec(durable_spec(&member))
        .await
        .expect("spawn a durable-identity member");
    let marker = format!("member history marker {}", Uuid::new_v4().simple());
    fixture.run_turn(&member, &marker).await;
    let session = handle
        .resolve_bridge_session_id(&member)
        .await
        .expect("a session-backed member");
    let first_generation = handle
        .get_member(&member)
        .await
        .expect("read the member")
        .expect("the member is present")
        .generation;

    let live = handle
        .last_member_binding(&member)
        .await
        .expect("read the history")
        .expect("a spawned member has a binding");
    assert_eq!(live.identity(), &member);
    assert_eq!(live.session_id(), Some(&session));
    assert_eq!(live.generation(), first_generation);
    assert_eq!(live.state(), MemberBindingState::Live);
    assert_eq!(live.role(), &ProfileName::from("lead"));
    assert!(matches!(
        live.continuity_intent(),
        SpawnContinuityIntent::DurableIdentity { .. }
    ));
    assert_eq!(
        live.identity_management(),
        MemberIdentityManagement::CommandDriven
    );
    assert_eq!(
        handle
            .member_binding_for_session(&session)
            .await
            .expect("read the history"),
        Some(live.clone())
    );
    assert!(
        handle
            .durable_identity_members()
            .await
            .expect("read the history")
            .contains(&live)
    );

    // `retire` answers a slow but healthy retirement with a typed
    // in-progress error once its budget elapses, which a loaded run reaches
    // in the session-archive stage; join the saga to its terminal reply.
    retire_to_terminal(handle, &member)
        .await
        .expect("retire the member to its terminal");

    let retired = handle
        .last_member_binding(&member)
        .await
        .expect("read the history")
        .expect("a retired member keeps its last binding");
    assert_eq!(retired.session_id(), Some(&session));
    assert_eq!(retired.generation(), first_generation);
    assert_eq!(retired.state(), MemberBindingState::Retired);
    assert_eq!(
        handle
            .member_binding_for_session(&session)
            .await
            .expect("read the history"),
        Some(retired.clone())
    );
    assert!(
        handle
            .durable_identity_members()
            .await
            .expect("read the history")
            .iter()
            .all(|binding| binding.identity() != &member),
        "a retired member is not a live durable-identity member"
    );
    assert!(
        handle
            .member_binding_history()
            .await
            .expect("read the history")
            .durable_identity_bindings()
            .any(|binding| binding == &retired),
        "a retired durable identity keeps its last binding among the durable identities"
    );

    assert_eq!(
        wake(handle, &member).await,
        session,
        "the wake resumes the session the history names"
    );
    let transcript = fixture.transcript(&session).await;
    assert!(
        transcript.contains(&marker) && transcript.contains("history-answer"),
        "the resumed member keeps the turn it ran before the retire: {transcript}"
    );

    let woken = handle
        .last_member_binding(&member)
        .await
        .expect("read the history")
        .expect("the woken member has a binding");
    assert_eq!(woken.session_id(), Some(&session));
    assert_eq!(woken.state(), MemberBindingState::Live);
    assert!(
        woken.generation() > first_generation,
        "Core mints the woken incarnation's generation"
    );
    assert_eq!(
        handle
            .member_binding_for_session(&session)
            .await
            .expect("read the history"),
        Some(woken.clone()),
        "a session answers its latest binding"
    );
    let durable = handle
        .durable_identity_members()
        .await
        .expect("read the history");
    assert!(durable.contains(&woken));

    let mob_db = fixture.mob_db();
    let _root = fixture.stop().await;
    let stored = MobStorage::read_member_binding_history(&mob_db)
        .await
        .expect("read the stopped mob's history");
    assert_eq!(stored.last_binding(&member), Some(&woken));
    assert_eq!(stored.binding_for_session(&session), Some(&woken));
    assert_eq!(
        stored
            .durable_identity_members()
            .cloned()
            .collect::<Vec<_>>(),
        durable
    );
}

// Only live members whose spawn carried the durable-identity marker
// are durable-identity members; an ordinary spawn, a helper and a fork are
// not, on the running mob and on its stopped store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_identity_members_lists_only_live_durable_identity_spawns() {
    let fixture = Fixture::new("member-history-durable").await;
    let handle = &fixture.handle;
    let durable = unique_identity("durable");
    let plain = unique_identity("plain");
    let helper = unique_identity("helper");
    let fork = unique_identity("fork");
    handle
        .spawn_spec(durable_spec(&durable))
        .await
        .expect("spawn the durable-identity member");
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", plain.clone()))
        .await
        .expect("spawn an ordinary member");
    handle
        .spawn_spec_internal_with_source(
            SpawnMemberSpec::new("lead", helper.clone()),
            SpawnSource::HelperSpawn,
        )
        .await
        .expect("spawn a helper that stays live");
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", fork.clone()).with_launch_mode(
            crate::launch::MemberLaunchMode::Fork {
                source_member_id: durable.clone(),
                fork_context: crate::launch::ForkContext::FullHistory,
            },
        ))
        .await
        .expect("fork the durable member");

    let history = handle
        .member_binding_history()
        .await
        .expect("read the history");
    assert_eq!(
        history
            .durable_identity_members()
            .map(|binding| binding.identity().clone())
            .collect::<Vec<_>>(),
        vec![durable.clone()]
    );
    for identity in [&plain, &helper, &fork] {
        let binding = history
            .last_binding(identity)
            .expect("a live member has a binding");
        assert_eq!(binding.state(), MemberBindingState::Live);
        assert_eq!(
            binding.continuity_intent(),
            &SpawnContinuityIntent::Ephemeral,
            "{identity} was not spawned as a durable identity"
        );
    }

    let mob_db = fixture.mob_db();
    let _root = fixture.stop().await;
    let stored = MobStorage::read_member_binding_history(&mob_db)
        .await
        .expect("read the stopped mob's history");
    assert_eq!(
        stored
            .durable_identity_members()
            .map(|binding| binding.identity().clone())
            .collect::<Vec<_>>(),
        vec![durable]
    );
}

// A spawn of a durable profile is a durable-identity member keyed by its
// identity, with no continuity intent on the spawn; an explicit intent on a
// spawn of the same profile keeps its own key. The stopped mob's store
// answers the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_profile_spawns_durable_identity_members() {
    let mut definition = turn_driven_definition("member-history-durable-profile");
    let mut worker = definition
        .profiles
        .get(&ProfileName::from("lead"))
        .and_then(ProfileBinding::as_inline)
        .expect("inline lead")
        .clone();
    worker.durable = true;
    definition.profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(worker)),
    );
    let fixture = Fixture::create_with(definition, None).await;
    let handle = &fixture.handle;
    let by_profile = unique_identity("by-profile");
    let explicit = unique_identity("explicit");
    let plain = unique_identity("plain");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", by_profile.clone()))
        .await
        .expect("spawn a member of the durable profile");
    let mut explicit_spec = SpawnMemberSpec::new("worker", explicit.clone());
    explicit_spec.continuity_intent = SpawnContinuityIntent::DurableIdentity {
        continuity_key: "work-item-7".to_string(),
    };
    handle
        .spawn_spec(explicit_spec)
        .await
        .expect("spawn the durable profile with an explicit intent");
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", plain.clone()))
        .await
        .expect("spawn an ordinary member");

    let expected = |history: &crate::runtime::MemberBindingHistory| {
        let mut durable = history
            .durable_identity_members()
            .map(|binding| {
                (
                    binding.identity().clone(),
                    binding.role().clone(),
                    binding.continuity_intent().clone(),
                )
            })
            .collect::<Vec<_>>();
        durable.sort_by(|a, b| a.0.cmp(&b.0));
        let mut want = vec![
            (
                by_profile.clone(),
                ProfileName::from("worker"),
                SpawnContinuityIntent::DurableIdentity {
                    continuity_key: by_profile.to_string(),
                },
            ),
            (
                explicit.clone(),
                ProfileName::from("worker"),
                SpawnContinuityIntent::DurableIdentity {
                    continuity_key: "work-item-7".to_string(),
                },
            ),
        ];
        want.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(durable, want);
        assert_eq!(
            history
                .last_binding(&plain)
                .expect("a live member has a binding")
                .continuity_intent(),
            &SpawnContinuityIntent::Ephemeral
        );
    };
    expected(
        &handle
            .member_binding_history()
            .await
            .expect("read the history"),
    );

    let mob_db = fixture.mob_db();
    let _root = fixture.stop().await;
    expected(
        &MobStorage::read_member_binding_history(&mob_db)
            .await
            .expect("read the stopped mob's history"),
    );
}

// Each binding carries the labels its incarnation was spawned with: a
// respawn's successor keeps them, the predecessor's retired binding and a
// retired member's last binding keep them, and the stopped mob's store
// answers the same. A member spawned without labels has none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_binding_carries_the_labels_its_incarnation_was_spawned_with() {
    let fixture = Fixture::new("member-history-labels").await;
    let handle = &fixture.handle;
    let labelled = unique_identity("labelled");
    let plain = unique_identity("plain");
    let labels = std::collections::BTreeMap::from([
        ("agent_identity".to_string(), "host:labelled".to_string()),
        ("team".to_string(), "north".to_string()),
    ]);
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", labelled.clone()).with_labels(labels.clone()))
        .await
        .expect("spawn the labelled member");
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", plain.clone()))
        .await
        .expect("spawn a member without labels");
    let first_session = handle
        .resolve_bridge_session_id(&labelled)
        .await
        .expect("a session-backed member");

    handle
        .respawn(labelled.clone(), None)
        .await
        .expect("respawn the labelled member");
    let history = handle
        .member_binding_history()
        .await
        .expect("read the history");
    let successor = history
        .last_binding(&labelled)
        .expect("the successor's binding");
    assert_eq!(
        (successor.state(), successor.labels()),
        (MemberBindingState::Live, &labels),
        "the successor keeps the spawn labels"
    );
    let predecessor = history
        .binding_for_session(&first_session)
        .expect("the predecessor's binding");
    assert_eq!(
        (predecessor.state(), predecessor.labels()),
        (MemberBindingState::Retired, &labels)
    );
    assert!(
        history
            .last_binding(&plain)
            .expect("the plain member's binding")
            .labels()
            .is_empty()
    );

    handle
        .retire(labelled.clone())
        .await
        .expect("retire the labelled member");
    let retired = handle
        .last_member_binding(&labelled)
        .await
        .expect("read the history")
        .expect("a retired binding");
    assert_eq!(
        (retired.state(), retired.labels()),
        (MemberBindingState::Retired, &labels)
    );

    let mob_db = fixture.mob_db();
    let _root = fixture.stop().await;
    let stored = MobStorage::read_member_binding_history(&mob_db)
        .await
        .expect("read the stopped mob's history");
    assert_eq!(
        stored
            .last_binding(&labelled)
            .map(|binding| binding.labels()),
        Some(&labels)
    );
    assert_eq!(
        stored
            .last_binding(&plain)
            .map(|binding| binding.labels().is_empty()),
        Some(true)
    );
}

// A mob reset ends every membership. The identity has no last binding
// after it, so the wake is a fresh spawn on a new session, while the
// pre-reset session still resolves to its retired binding.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_reset_makes_the_next_wake_fresh_and_keeps_old_sessions_resolvable() {
    let fixture = Fixture::new("member-history-reset").await;
    let handle = &fixture.handle;
    let member = unique_identity("reset-member");
    let old_session = wake(handle, &member).await;

    handle.reset().await.expect("reset the mob");

    assert_eq!(
        handle
            .last_member_binding(&member)
            .await
            .expect("read the history"),
        None,
        "a mob reset leaves the identity no last binding"
    );
    assert_eq!(
        handle
            .member_binding_history()
            .await
            .expect("read the history")
            .durable_identity_bindings()
            .count(),
        0,
        "a mob reset ends every durable identity's last binding"
    );
    let pre_reset = handle
        .member_binding_for_session(&old_session)
        .await
        .expect("read the history")
        .expect("a pre-reset session still resolves");
    assert_eq!(pre_reset.identity(), &member);
    assert_eq!(pre_reset.state(), MemberBindingState::Retired);

    let fresh_session = wake(handle, &member).await;
    assert_ne!(
        fresh_session, old_session,
        "the wake after a reset starts a new session"
    );
    assert_eq!(
        handle
            .last_member_binding(&member)
            .await
            .expect("read the history")
            .map(|binding| (binding.session_id().cloned(), binding.state())),
        Some((Some(fresh_session), MemberBindingState::Live))
    );
    assert_eq!(
        handle
            .member_binding_for_session(&old_session)
            .await
            .expect("read the history")
            .map(|binding| binding.state()),
        Some(MemberBindingState::Retired)
    );
    fixture.stop().await;
}

// Retention keeps every event the history reads: a prune with a cutoff a
// day ahead removes everything else and the history still answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_history_survives_a_prune_of_everything_else() {
    let fixture = Fixture::new("member-history-prune").await;
    let handle = &fixture.handle;
    let member = unique_identity("pruned-member");
    let session = wake(handle, &member).await;
    retire_to_terminal(handle, &member)
        .await
        .expect("retire the member to its terminal");
    let mob_id = handle.mob_id().clone();
    let mob_db = fixture.mob_db();
    let _root = fixture.stop().await;

    let storage = MobStorage::persistent(&mob_db).expect("reopen the mob store");
    storage
        .events
        .append(crate::event::NewMobEvent::for_test(
            mob_id,
            crate::event::MobEventKind::OrchestratorResumeNotificationFailed {
                member_id: member.clone(),
                session_id: session.clone(),
                reason: "a non-anchor event the prune removes".to_string(),
            },
        ))
        .await
        .expect("append a non-anchor event");
    let removed = storage
        .events
        .prune(chrono::Utc::now() + chrono::Duration::days(1))
        .await
        .expect("prune a day ahead");
    assert!(removed >= 1, "the prune removed the non-anchor events");
    drop(storage);

    let stored = MobStorage::read_member_binding_history(&mob_db)
        .await
        .expect("read the pruned history");
    let binding = stored
        .last_binding(&member)
        .expect("the member's binding survives the prune");
    assert_eq!(binding.session_id(), Some(&session));
    assert_eq!(binding.state(), MemberBindingState::Retired);
    assert_eq!(stored.binding_for_session(&session), Some(binding));
}

// A host-adopted member reads intent-managed from the moment its intent row
// commits, in the same read as its binding, and stays live on its session; a
// command-driven durable member and an identity that never spawned read
// command-driven; the stopped store answers the same. The read needs only the
// committed intent row, so this test does not wait for Core to converge the
// adopted identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_adopted_member_reads_intent_managed() {
    let fixture =
        Fixture::create("member-history-adopted", Some("member history base prompt")).await;
    let handle = &fixture.handle;
    let adopted = unique_identity("adopted");
    let commanded = unique_identity("commanded");
    handle
        .spawn_spec(SpawnMemberSpec::new("lead", adopted.clone()))
        .await
        .expect("spawn the member a host adopts");
    handle
        .spawn_spec(durable_spec(&commanded))
        .await
        .expect("spawn a command-driven durable member");
    let session = handle
        .resolve_bridge_session_id(&adopted)
        .await
        .expect("a session-backed member");
    assert_eq!(
        handle
            .identity_management(&adopted)
            .await
            .expect("read the history"),
        MemberIdentityManagement::CommandDriven,
        "no intent row before the adoption"
    );

    adopt_existing_member(handle, &adopted, &session, "lead").await;

    let history = handle
        .member_binding_history()
        .await
        .expect("read the history");
    let adopted_binding = history
        .last_binding(&adopted)
        .expect("the adopted member has a binding");
    assert_eq!(
        adopted_binding.identity_management(),
        MemberIdentityManagement::IntentManaged
    );
    assert_eq!(adopted_binding.state(), MemberBindingState::Live);
    assert_eq!(adopted_binding.session_id(), Some(&session));
    assert_eq!(
        history.identity_management(&commanded),
        MemberIdentityManagement::CommandDriven
    );
    assert_eq!(
        history.identity_management(&unique_identity("never-spawned")),
        MemberIdentityManagement::CommandDriven
    );

    let mob_db = fixture.mob_db();
    let _root = fixture.stop().await;
    let stored = MobStorage::read_member_binding_history(&mob_db)
        .await
        .expect("read the stopped mob's history");
    assert_eq!(
        stored
            .last_binding(&adopted)
            .map(|binding| (binding.session_id().cloned(), binding.identity_management())),
        Some((Some(session), MemberIdentityManagement::IntentManaged))
    );
    assert_eq!(
        stored.identity_management(&commanded),
        MemberIdentityManagement::CommandDriven
    );
    assert!(
        stored.durable_identity_members().any(|binding| {
            binding.identity() == &commanded
                && binding.identity_management() == MemberIdentityManagement::CommandDriven
        }),
        "the command-driven durable member is listed with its fact"
    );
}

// When Core realizes an adopted identity again from its intent row, it binds
// the identity as a durable-identity member on the same session, so the
// durable-identity list holds it as intent-managed beside a command-driven
// durable member and a roster compile can leave it alone. The realization is
// the same actor path for every store; this test runs it over in-memory
// stores.
#[tokio::test]
async fn core_realizes_an_adopted_member_as_a_durable_identity_member() {
    let session_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        PersistentMockBuilder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let definition = with_unique_mob_id(sample_definition(), "member-history-realized");
    let mob_id = definition.id.clone();
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_spawn_base_prompt_source(Arc::new(crate::StaticSpawnBasePromptSource(
            "member history base prompt".to_string(),
        )))
        .create()
        .await
        .expect("create the mob");
    let adopted = unique_identity("adopted");
    let commanded = unique_identity("commanded");
    let mut adopted_spec = SpawnMemberSpec::new("worker", adopted.clone());
    adopted_spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    handle
        .spawn_spec(adopted_spec)
        .await
        .expect("spawn the member a host adopts");
    let mut commanded_spec = SpawnMemberSpec::new("worker", commanded.clone());
    commanded_spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    commanded_spec.continuity_intent = SpawnContinuityIntent::DurableIdentity {
        continuity_key: commanded.to_string(),
    };
    handle
        .spawn_spec(commanded_spec)
        .await
        .expect("spawn a command-driven durable member");
    let session = handle
        .resolve_bridge_session_id(&adopted)
        .await
        .expect("a session-backed member");

    adopt_existing_member(&handle, &adopted, &session, "worker").await;
    await_adoption_convergence(&handle, &adopted).await;
    rematerialize_from_intent(&handle, service.as_ref(), &adopted, &session).await;

    let durable = handle
        .durable_identity_members()
        .await
        .expect("read the history");
    assert_eq!(
        durable
            .iter()
            .map(|binding| (binding.identity().clone(), binding.identity_management()))
            .collect::<Vec<_>>(),
        vec![
            (adopted.clone(), MemberIdentityManagement::IntentManaged),
            (commanded.clone(), MemberIdentityManagement::CommandDriven),
        ],
        "Core's realization of an intent row carries the durable-identity marker"
    );
    let realized = durable
        .iter()
        .find(|binding| binding.identity() == &adopted)
        .expect("the realized member is listed");
    assert_eq!(realized.session_id(), Some(&session));
    assert_eq!(realized.state(), MemberBindingState::Live);

    tokio::time::timeout(WAIT, handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    drop(handle);
    await_supervisor_route_release(&mob_id).await;
}

/// Adopt an existing member's declaration through Core's identity intent
/// rows, as a host does. The intent row is committed when this returns.
async fn adopt_existing_member(
    handle: &MobHandle,
    member: &AgentIdentity,
    session: &SessionId,
    profile: &str,
) {
    let adoption = crate::identity::AdoptMemberIdentityDeclaration {
        mob_id: handle.mob_id().clone(),
        agent_identity: member.clone(),
        request_id: crate::identity::IdentityAdoptionId::new(format!("adopt-{member}"))
            .expect("valid adoption id"),
        precondition: crate::identity::IdentityAdoptionPrecondition::ExpectedAbsent,
        declaration_scope: crate::identity::IdentityDeclarationScopeId::new("history-policy")
            .expect("valid declaration scope"),
        declaration_revision: 1,
        session: crate::identity::DesiredSessionTarget {
            session_id: session.clone(),
            lineage_id: meerkat_core::SessionLineageId::for_session(session),
            lineage_generation: meerkat_core::SessionGeneration::INITIAL,
            authority_policy: crate::identity::DesiredSessionAuthorityPolicy::RequireExisting,
        },
        member: crate::identity::IdentityProfileMemberDeclaration {
            profile_name: ProfileName::from(profile),
            profile_override: None,
            model_override: None,
            external_addressable_override: None,
            context: None,
            labels: None,
            additional_instructions: None,
            system_prompt_override: None,
            tool_access_policy: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: Some(meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven),
            required_env_keys: Vec::new(),
            required_local_callback_tools: Vec::new(),
            execution: crate::identity::DesiredExecution::ControllingSession,
        },
        wiring_custody: crate::identity::IdentityWiringCustody::ExternalManaged,
        owned_wiring: BTreeSet::new(),
        convergence: crate::identity::IdentityConvergenceMode::Drain { max_wait_ms: 5_000 },
    };
    assert!(matches!(
        handle
            .adopt_member_identity_declaration(adoption)
            .await
            .expect("adopt the existing member")
            .adoption,
        crate::identity::IdentityAdoptionOutcome::Adopted {
            desired_revision: 1
        }
    ));
}

/// Wait until Core converges the adopted identity's first intent revision.
async fn await_adoption_convergence(handle: &MobHandle, member: &AgentIdentity) {
    let converged = tokio::time::timeout(WAIT, async {
        loop {
            if matches!(
                handle
                    .identity_convergence_status(member)
                    .await
                    .expect("read the adoption convergence"),
                crate::identity::IdentityStoredObservation::Valid(
                    crate::identity::IdentityConvergenceStatus {
                        intent_revision: Some(1),
                        active_intent_revision: Some(1),
                        decision: Some(crate::identity::IdentityReconcileDecision::Converged),
                        ..
                    }
                )
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        converged.is_ok(),
        "the adoption converges; latest status: {:?}",
        handle
            .identity_convergence_status(member)
            .await
            .expect("read the adoption convergence")
    );
}

/// Change the adopted identity's declared tools, so Core's reconciler
/// realizes the member again from its intent row on the same session, and
/// wait for that realization.
async fn rematerialize_from_intent<S>(
    handle: &MobHandle,
    service: &S,
    member: &AgentIdentity,
    session: &SessionId,
) where
    S: MobSessionService + ?Sized,
{
    let record = match handle
        .identity_intent(member)
        .await
        .expect("read the identity intent")
    {
        crate::identity::IdentityStoredObservation::Valid(record) => record,
        other => panic!("expected the adopted identity intent, got {other:?}"),
    };
    let mut declaration = match &record.intent {
        crate::identity::IdentityIntent::Present { member, .. } => {
            member.material.member_tool_declaration()
        }
        crate::identity::IdentityIntent::Absent { .. } => {
            panic!("the adoption persists a present intent")
        }
    };
    declaration.category_overrides.shell = ToolCategoryOverride::Enable;
    let update = handle
        .apply_member_tool_declaration(crate::identity::ApplyMemberToolDeclaration {
            mob_id: handle.mob_id().clone(),
            agent_identity: member.clone(),
            request_id: crate::identity::MemberToolMutationId::new(format!("shell-{member}"))
                .expect("valid mutation id"),
            expected_intent_revision: 1,
            declaration,
            convergence: crate::identity::IdentityConvergenceMode::CancelActive,
        })
        .await
        .expect("apply the tool declaration");
    assert!(matches!(
        update.commit,
        crate::identity::MemberToolCommitOutcome::Committed {
            desired_revision: 2
        }
    ));
    let realized = tokio::time::timeout(WAIT, async {
        loop {
            let shell = MobSessionService::load_persisted_session(service, session)
                .await
                .expect("read the realized session")
                .and_then(|session| session.session_metadata())
                .map(|metadata| metadata.tooling.shell);
            if shell == Some(ToolCategoryOverride::Enable)
                && handle.resolve_bridge_session_id(member).await.as_ref() == Some(session)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        realized.is_ok(),
        "Core realizes the new declaration on the same session; latest status: {:?}",
        handle
            .identity_convergence_status(member)
            .await
            .expect("read the realization convergence")
    );
}

// The store-level reader refuses a missing store and creates nothing.
#[tokio::test]
async fn the_store_level_reader_refuses_a_missing_store_and_creates_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("mob.db");
    let refused = MobStorage::read_member_binding_history(&missing).await;
    assert!(
        matches!(
            &refused,
            Err(MemberBindingHistoryReadError::StoreMissing { path }) if path == &missing
        ),
        "a missing store is refused typed: {refused:?}"
    );
    assert!(!missing.exists(), "the reader never creates a store");
}

// The store-level reader refuses a file that is not a mob store this build
// reads and leaves it byte for byte as it was.
#[tokio::test]
async fn the_store_level_reader_refuses_foreign_files_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let not_sqlite = dir.path().join("not-sqlite.db");
    std::fs::write(&not_sqlite, b"this file is not a SQLite database").expect("write");
    let no_mob_schema = dir.path().join("no-mob-schema.db");
    rusqlite::Connection::open(&no_mob_schema)
        .expect("open")
        .execute_batch("CREATE TABLE unrelated (value INTEGER);")
        .expect("unrelated schema");
    let future_schema = dir.path().join("future-schema.db");
    rusqlite::Connection::open(&future_schema)
        .expect("open")
        .execute_batch(
            "CREATE TABLE meerkat_schema (
                 domain TEXT PRIMARY KEY,
                 version INTEGER NOT NULL
             );
             INSERT INTO meerkat_schema VALUES ('mob', 9999);",
        )
        .expect("future ledger");
    for path in [&not_sqlite, &no_mob_schema, &future_schema] {
        let before = std::fs::read(path).expect("read before");
        let refused = MobStorage::read_member_binding_history(path).await;
        assert!(
            matches!(
                &refused,
                Err(MemberBindingHistoryReadError::ForeignSchema { path: refused_path, .. })
                    if refused_path == path
            ),
            "{} is refused as a foreign schema: {refused:?}",
            path.display()
        );
        assert_eq!(
            std::fs::read(path).expect("read after"),
            before,
            "{} is left as it was",
            path.display()
        );
    }
}
