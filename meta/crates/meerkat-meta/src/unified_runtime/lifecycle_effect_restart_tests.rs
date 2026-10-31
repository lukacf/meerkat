//! The side effects of an identity's lifecycle steps across a kill: the
//! distillation of the session a reset retires, the host's retirement hook,
//! the customizer's `after_create` and the exit interview a roster owes the
//! identities it no longer lists.
//!
//! Each effect follows a lifecycle step Core journals (a rotation, a seat, a
//! retirement). A process killed between the step and its effect still owes
//! the effect: the next start over the same state delivers it, once, and
//! repeats none that the killed process completed, nor one for an
//! incarnation that ended since. An effect that never answers, or whose
//! write always fails, is reported and delays the effects after it by at most
//! its bound.
//!
//! A kill is a child process of this test binary: the test's own body,
//! selected by [`CHILD_STATE_ENV`], boots over a state directory, takes the
//! step, and ends the process from inside the effect under test, with no
//! shutdown. The parent then boots over the same directory and checks what
//! its start delivered.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::types::SessionId;

use crate::identity_first::runtime::RetiredMemberObserver;
use crate::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentIdentity,
    AgentMemoryConfig, AgentMemoryRuntimeInjector, AgentRuntimeServices, ContinuityStore,
    CustomizerError, DurabilityPolicy, DurableAgentSpec, FirstBootManifest, FirstBootRow,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
    LocalLeaseProvider, MobSessionBridge, MutableRosterProvider, RetiredIdentityMember,
    RosterProvider,
};
use crate::memory::StewardStore;
use crate::memory::distiller::{
    DistillerClientHandle, DistillerConfig, DistillerEngine, DistillerError, DistillerProfile,
    TranscriptSlice, TranscriptSource,
};
use crate::memory::steward::{
    StewardClientHandle, StewardConfig, StewardEngine, StewardError, StewardProfile,
};
use crate::mob_handle_runtime::SessionCreatedContext;
use crate::unified_runtime::ErrorHook;
use crate::unified_runtime::types::ErrorEvent;
use crate::{
    ChildMobStorage, DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    PersistentMetadataStore, RuntimeOptions, SqliteAgentMemoryStore, SqliteMetadataStore,
    UnifiedRuntime,
};

/// Set in a child process of this binary: the state directory its test's
/// child body boots over.
const CHILD_STATE_ENV: &str = "META_LIFECYCLE_EFFECT_CHILD_STATE";

/// How long a child may run before the parent kills it: the child ends
/// itself from inside the effect under test, so one still running after this
/// never reached the effect.
const CHILD_DEADLINE: Duration = Duration::from_mins(3);

/// How long the parent waits for an effect its start owes.
const EFFECT_DEADLINE: Duration = Duration::from_mins(1);

/// How long the parent waits, after an awaited effect, for one that must not
/// come: long enough for a repeated effect of the same start to show.
const SETTLE: Duration = Duration::from_millis(500);

/// The agent memory realm of the fixture's distiller and steward.
const REALM: &str = "default";

const ALPHA: &str = "alpha-1";
const BETA: &str = "beta-1";
const GAMMA: &str = "gamma-1";

/// The bounds of each lifecycle side effect in the rows that wait for one:
/// an attempt that has not answered within this deadline failed.
const SHORT_DEADLINE: Duration = Duration::from_millis(200);

/// How many attempts of one side effect those rows allow.
const SHORT_ATTEMPTS: u32 = 2;

fn mob_toml(mob_id: &str) -> String {
    format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.lead]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.lead.tools]\ncomms = true\n"
    )
}

fn member(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(name).expect("identity"),
        profile: meerkat_mob::ProfileName::from("lead"),
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

/// End this process at once, with no shutdown: the kill of the rows below.
fn exit_now<T>() -> T {
    std::process::exit(0)
}

/// The state directory of this process's child body, when it is a child.
fn child_state() -> Option<PathBuf> {
    std::env::var_os(CHILD_STATE_ENV).map(PathBuf::from)
}

/// `name`, a test of this module, as the harness filters it in this binary.
fn test_path(name: &str) -> String {
    let module = module_path!()
        .split_once("::")
        .map_or("", |(_crate_name, module)| module);
    format!("{module}::{name}")
}

/// Run `name`'s child body over `state` in a child process of this binary
/// and wait for it to end: it ends itself from inside the effect under test.
async fn run_child(name: &str, state: &Path) {
    let binary = std::env::current_exe().expect("locate the test binary");
    let status = tokio::time::timeout(
        CHILD_DEADLINE,
        tokio::process::Command::new(binary)
            .args([
                test_path(name).as_str(),
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_STATE_ENV, state)
            .kill_on_drop(true)
            .status(),
    )
    .await
    .unwrap_or_else(|_| panic!("the child of {name} never reached the effect under test"))
    .expect("run the child process");
    assert!(status.success(), "the child of {name} failed: {status}");
}

/// What a boot installs on its identity runtime before the roster pass.
#[derive(Default)]
struct Wiring {
    observer: Option<RetiredMemberObserver>,
    customizer: Option<Arc<dyn AgentCustomizer>>,
    memory: Option<AgentMemoryRuntimeInjector>,
    /// Read the first-boot manifest from the boot's state root.
    first_boot_rows: bool,
    error_hook: Option<ErrorHook>,
    /// Bound each lifecycle side effect: a deadline per attempt, and the
    /// attempts allowed.
    effect_limits: Option<(Duration, u32)>,
}

/// One boot over a state directory: a persistent mob, sessions and metadata,
/// with its identity runtime installed and its roster bootstrapped.
struct Boot {
    runtime: UnifiedRuntime,
    identities: Arc<IdentityRuntime>,
    context: Arc<IdentityFirstRuntimeContext>,
    roster: Arc<MutableRosterProvider>,
}

impl Boot {
    async fn session_of(&self, identity: &AgentIdentity) -> SessionId {
        self.identities
            .status(identity)
            .await
            .expect("the identity's status")
            .session_id
            .expect("the identity is bound to a session")
    }

    async fn generation_of(&self, identity: &AgentIdentity) -> u64 {
        self.identities
            .status(identity)
            .await
            .expect("the identity's status")
            .generation
            .expect("the identity has a generation")
            .get()
    }
}

/// Boot over `state`: the mob's journal in `mob.sqlite3`, its sessions and
/// the identity continuity under `state/state`, Meta's metadata (the
/// identity records) in `meta.sqlite3`. The identity runtime is composed as
/// the gateways compose it, with `wiring` installed before the roster pass,
/// and the customizer handed to the identity context, as a host does.
async fn boot(state: &Path, mob_id: &str, roster: Vec<DurableAgentSpec>, wiring: Wiring) -> Boot {
    let state_root = state.join("state");
    std::fs::create_dir_all(&state_root).expect("state root");
    let definition =
        meerkat_mob::MobDefinition::from_toml(&mob_toml(mob_id)).expect("mob definition");
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(state.join("mob.sqlite3"))
            .expect("open persistent mob storage");
    let spec =
        MobBootstrapSpec::persistent(definition, storage, provenance, state_root.clone(), 16)
            .await
            .expect("compose persistent Meta stores")
            .with_child_mob_storage(ChildMobStorage::ProcessBound)
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
            });
    let metadata: Arc<dyn PersistentMetadataStore> = Arc::new(
        SqliteMetadataStore::open(state.join("meta.sqlite3")).expect("open the metadata store"),
    );
    let mut runtime = UnifiedRuntime::bootstrap_with_options(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: mob_id.to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        Duration::from_secs(2),
        RuntimeOptions::default(),
        metadata,
    )
    .await
    .expect("bootstrap the unified runtime");
    let continuity = Arc::new(
        LocalContinuityStore::open(state_root.join("identity-continuity.sqlite3"))
            .expect("open the identity continuity store"),
    );
    let identities = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: continuity as Arc<dyn ContinuityStore>,
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: format!("{mob_id}-instance"),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(
                MobSessionBridge::with_session_service(
                    crate::gateway_wiring::identity_mob_handle(&runtime),
                    runtime
                        .mob_runtime()
                        .session_service()
                        .cloned()
                        .expect("the persistent runtime has a session service"),
                )
                .with_controller_pin_source(
                    runtime
                        .mob_runtime()
                        .controller_pin_source()
                        .expect("the persistent runtime records its session owner"),
                ),
            )),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(
            crate::gateway_wiring::identity_mob_handle(&runtime),
        )),
    );
    // As the builder composes it: the identity records live in the
    // composition's metadata store.
    identities.set_identity_records(
        Arc::clone(runtime.persistent_metadata()),
        runtime.mob_handle().mob_id().as_str().to_string(),
    );
    if let Some(observer) = wiring.observer {
        identities.set_retired_member_observer(Some(observer));
    }
    if let Some(memory) = wiring.memory {
        identities.set_agent_memory(Some(memory)).await;
    }
    if wiring.first_boot_rows {
        identities.set_first_boot_state_dir(Some(state_root));
    }
    if let Some(hook) = wiring.error_hook {
        identities.set_error_hook(Some(hook));
    }
    if let Some((deadline, attempts)) = wiring.effect_limits {
        identities.set_lifecycle_effect_limits(deadline, attempts);
    }
    let roster_provider = Arc::new(MutableRosterProvider::new(roster.clone()));
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        Arc::clone(&identities),
        Arc::clone(&roster_provider) as Arc<dyn RosterProvider>,
        None,
        wiring.customizer,
        Some(runtime.mob_handle().definition().clone()),
    ));
    runtime
        .install_and_bootstrap_identity_first_context(Arc::clone(&context), &roster)
        .await
        .expect("bootstrap the identity roster");
    Boot {
        runtime,
        identities,
        context,
        roster: roster_provider,
    }
}

/// A model client handle for the distiller and the steward of the fixture.
/// Neither ever needs an answer here: the rows observe the transcript read
/// and the exit-interview queue, not a model's output.
struct TestClientHandle;

#[async_trait::async_trait]
impl DistillerClientHandle for TestClientHandle {
    async fn client(&self) -> Result<Arc<dyn meerkat_client::LlmClient>, DistillerError> {
        Ok(Arc::new(meerkat_client::TestClient::default()))
    }

    fn invalidate(&self) {}
}

#[async_trait::async_trait]
impl StewardClientHandle for TestClientHandle {
    async fn client(&self) -> Result<Arc<dyn meerkat_client::LlmClient>, StewardError> {
        Ok(Arc::new(meerkat_client::TestClient::default()))
    }

    fn invalidate(&self) {}
}

/// A transcript source that ends the process when a session is read: the
/// kill lands inside the distillation.
struct ExitOnRead;

#[async_trait::async_trait]
impl TranscriptSource for ExitOnRead {
    async fn read(
        &self,
        _session_key: &str,
        _from_index: u64,
    ) -> Result<Option<TranscriptSlice>, DistillerError> {
        exit_now()
    }
}

/// A transcript source that records each session it is asked for and
/// answers that the store holds no such session, so no model is called.
#[derive(Default)]
struct RecordingTranscripts {
    reads: Mutex<Vec<String>>,
}

impl RecordingTranscripts {
    fn reads_of(&self, session: &SessionId) -> usize {
        let session = session.to_string();
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|read| **read == session)
            .count()
    }
}

#[async_trait::async_trait]
impl TranscriptSource for RecordingTranscripts {
    async fn read(
        &self,
        session_key: &str,
        _from_index: u64,
    ) -> Result<Option<TranscriptSlice>, DistillerError> {
        self.reads.lock().unwrap().push(session_key.to_string());
        Ok(None)
    }
}

/// Agent memory over the store in `dir`, with a distiller that reads
/// sessions through `transcripts` and a steward that keeps the exit-interview
/// queue in the same store, and that store.
fn memory_over(
    dir: &Path,
    transcripts: Arc<dyn TranscriptSource>,
) -> (AgentMemoryRuntimeInjector, Arc<SqliteAgentMemoryStore>) {
    std::fs::create_dir_all(dir).expect("memory dir");
    let store = Arc::new(SqliteAgentMemoryStore::open(dir).expect("open the memory store"));
    let distiller = Arc::new(DistillerEngine::new(
        DistillerProfile::embedded_default(),
        DistillerConfig {
            enabled: true,
            ..DistillerConfig::default()
        },
        Arc::new(TestClientHandle),
        store.clone(),
        store.clone(),
        Arc::clone(&transcripts),
        None,
        None,
        REALM,
    ));
    let steward = Arc::new(StewardEngine::new(
        StewardProfile::embedded_default(),
        StewardConfig {
            enabled: true,
            ..StewardConfig::default()
        },
        Arc::new(TestClientHandle),
        store.clone(),
        transcripts,
        REALM,
    ));
    let injector = AgentMemoryRuntimeInjector::new(store.clone(), AgentMemoryConfig::default())
        .with_distiller(distiller)
        .with_steward(steward);
    (injector, store)
}

/// [`memory_over`]'s agent memory alone.
fn memory_injector(
    dir: &Path,
    transcripts: Arc<dyn TranscriptSource>,
) -> AgentMemoryRuntimeInjector {
    memory_over(dir, transcripts).0
}

/// The exit interviews `store` holds pending for `identity`, each with its
/// cause.
async fn pending_harvests_of(
    store: &SqliteAgentMemoryStore,
    identity: &AgentIdentity,
) -> Vec<String> {
    store
        .pending_harvests(REALM, 64)
        .await
        .expect("read the exit-interview queue")
        .into_iter()
        .filter(|harvest| harvest.identity == identity.as_str())
        .map(|harvest| harvest.cause)
        .collect()
}

/// A customizer that changes no build and records each `after_create`.
#[derive(Default)]
struct RecordingCustomizer {
    told: Mutex<Vec<(AgentIdentity, SessionId)>>,
}

impl RecordingCustomizer {
    fn told(&self) -> Vec<(AgentIdentity, SessionId)> {
        self.told.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AgentCustomizer for RecordingCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }

    async fn after_create(
        &self,
        identity: &AgentIdentity,
        session_id: &SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        self.told
            .lock()
            .unwrap()
            .push((identity.clone(), session_id.clone()));
        Ok(())
    }
}

/// A customizer whose `after_create` ends the process: the kill lands inside
/// the notice.
struct ExitingAfterCreate;

#[async_trait::async_trait]
impl AgentCustomizer for ExitingAfterCreate {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        _session_id: &SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        exit_now()
    }
}

/// The host's retirement hook, recording each incarnation it hears of.
fn recording_observer(seen: &Arc<Mutex<Vec<RetiredIdentityMember>>>) -> RetiredMemberObserver {
    let seen = Arc::clone(seen);
    Arc::new(move |retired| {
        seen.lock().unwrap().push(retired);
        Box::pin(async { Ok(()) })
    })
}

/// The host's retirement hook, ending the process when it is called: the
/// kill lands inside the hook.
fn exiting_observer() -> RetiredMemberObserver {
    Arc::new(move |_retired| Box::pin(async { exit_now::<Result<(), String>>() }))
}

/// The host's retirement hook, recording each incarnation it hears of, that
/// never answers for an incarnation of `stuck`.
fn hook_stuck_on(
    stuck: &AgentIdentity,
    seen: &Arc<Mutex<Vec<RetiredIdentityMember>>>,
) -> RetiredMemberObserver {
    let stuck = stuck.clone();
    let seen = Arc::clone(seen);
    Arc::new(move |retired| {
        let never = retired.identity == stuck;
        seen.lock().unwrap().push(retired);
        Box::pin(async move {
            if never {
                std::future::pending::<()>().await;
            }
            Ok(())
        })
    })
}

/// The incarnations of `identity` the hook heard of, each with its cause.
fn hook_calls_of(
    seen: &Mutex<Vec<RetiredIdentityMember>>,
    identity: &AgentIdentity,
) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|retired| retired.identity == *identity)
        .map(|retired| retired.cause.as_str().to_string())
        .collect()
}

/// An error hook that keeps every event it is handed.
fn keeping_error_hook(events: &Arc<Mutex<Vec<ErrorEvent>>>) -> ErrorHook {
    let events = Arc::clone(events);
    Arc::new(move |event| {
        events.lock().unwrap().push(event);
        Box::pin(async {})
    })
}

/// The failed lifecycle side effects of `identity` the error hook was
/// handed, each in its wire form.
fn failed_effects_of(
    events: &Mutex<Vec<ErrorEvent>>,
    identity: &AgentIdentity,
) -> Vec<serde_json::Value> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|event| serde_json::to_value(event).expect("an error event serializes"))
        .filter(|event| {
            event["category"] == "lifecycle_effect_failed" && event["identity"] == identity.as_str()
        })
        .collect()
}

/// The failed releases of `identity` the error hook was handed (its
/// retirement hook answered an error, or never answered), each in its wire
/// form.
fn failed_releases_of(
    events: &Mutex<Vec<ErrorEvent>>,
    identity: &AgentIdentity,
) -> Vec<serde_json::Value> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|event| serde_json::to_value(event).expect("an error event serializes"))
        .filter(|event| {
            event["category"] == "retired_member_release_failed"
                && event["identity"] == identity.as_str()
        })
        .collect()
}

/// From now on, every write to the exit-interview queue in `store` fails.
async fn refuse_exit_interviews(store: &SqliteAgentMemoryStore) {
    // A read opens the realm's database, and its tables, first.
    store
        .pending_harvests(REALM, 1)
        .await
        .expect("read the exit-interview queue");
    let conn =
        rusqlite::Connection::open(store.path_for_realm(REALM)).expect("open the memory database");
    conn.execute_batch(
        "CREATE TRIGGER refuse_exit_interviews BEFORE INSERT ON pending_harvests \
         BEGIN SELECT RAISE(ABORT, 'the exit-interview queue refuses every write'); END;",
    )
    .expect("make the exit-interview queue refuse every write");
}

/// Wait until `ready` answers, polling: the parent's start owes the effect.
async fn eventually<T>(what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + EFFECT_DEADLINE;
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {EFFECT_DEADLINE:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A reset's fresh life commits in Core, and the process is killed while
/// the session the reset retired is being distilled. The next start over the
/// same state distills that session, once.
///
/// RED before lifecycle effects were projected from Core's journal: the
/// distillation was owed in an in-memory ledger, drained only by a graceful
/// shutdown, so a kill lost it and the next start distilled nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_kill_inside_a_reset_s_distillation_distills_the_retired_session_after_the_restart() {
    const NAME: &str =
        "a_kill_inside_a_reset_s_distillation_distills_the_retired_session_after_the_restart";
    const MOB: &str = "lifecycle-effect-reset-distill";
    let alpha = member(ALPHA);
    if let Some(state) = child_state() {
        let boot = boot(
            &state,
            MOB,
            vec![alpha.clone()],
            Wiring {
                memory: Some(memory_injector(&state.join("memory"), Arc::new(ExitOnRead))),
                ..Wiring::default()
            },
        )
        .await;
        let retired = boot.session_of(&alpha.identity).await;
        std::fs::write(state.join("retired-session"), retired.to_string())
            .expect("note the session the reset retires");
        boot.identities
            .reset_tracked(&alpha.identity)
            .await
            .expect("the reset commits its fresh life");
        // The distillation of the retired session ends the process.
        std::future::pending::<()>().await;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    run_child(NAME, temp.path()).await;
    let retired = SessionId::parse(
        &std::fs::read_to_string(temp.path().join("retired-session"))
            .expect("the child noted the retired session"),
    )
    .expect("a session id");

    let transcripts = Arc::new(RecordingTranscripts::default());
    let boot = boot(
        temp.path(),
        MOB,
        vec![alpha.clone()],
        Wiring {
            memory: Some(memory_injector(
                &temp.path().join("memory"),
                Arc::clone(&transcripts) as Arc<dyn TranscriptSource>,
            )),
            ..Wiring::default()
        },
    )
    .await;
    eventually("the retired session's distillation", || {
        (transcripts.reads_of(&retired) > 0).then_some(())
    })
    .await;
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        transcripts.reads_of(&retired),
        1,
        "the retired session is distilled once after the restart"
    );
    assert_ne!(
        boot.session_of(&alpha.identity).await,
        retired,
        "the fresh life runs on a fresh session"
    );
    let _ = boot.runtime.shutdown().await;
}

/// A first-boot `Fresh` row (the row an offline `identity-reset` writes)
/// gives a live identity its fresh life at the next start, as Core's
/// rotation, and the session that rotation retires is distilled as an
/// online reset's is.
///
/// RED before lifecycle effects were projected from Core's journal: only the
/// online reset verb captured its outgoing session, so a rotation the first
/// boot carried out distilled nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_boot_fresh_row_distills_the_retired_session_like_an_online_reset() {
    const MOB: &str = "lifecycle-effect-fresh-row";
    let temp = tempfile::tempdir().expect("temp dir");
    let alpha = member(ALPHA);
    let retired = {
        let boot = boot(temp.path(), MOB, vec![alpha.clone()], Wiring::default()).await;
        let session = boot.session_of(&alpha.identity).await;
        let _ = boot.runtime.shutdown().await;
        session
    };
    FirstBootManifest::upsert_row(
        &temp.path().join("state"),
        FirstBootRow::Fresh {
            identity: alpha.identity.clone(),
            retired_session: retired.clone(),
        },
    )
    .expect("write the identity's fresh row");

    let transcripts = Arc::new(RecordingTranscripts::default());
    let boot = boot(
        temp.path(),
        MOB,
        vec![alpha.clone()],
        Wiring {
            memory: Some(memory_injector(
                &temp.path().join("memory"),
                Arc::clone(&transcripts) as Arc<dyn TranscriptSource>,
            )),
            first_boot_rows: true,
            ..Wiring::default()
        },
    )
    .await;
    assert_ne!(
        boot.session_of(&alpha.identity).await,
        retired,
        "the row gave the identity its fresh life"
    );
    eventually("the retired session's distillation", || {
        (transcripts.reads_of(&retired) > 0).then_some(())
    })
    .await;
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        transcripts.reads_of(&retired),
        1,
        "the retired session is distilled once"
    );
    let _ = boot.runtime.shutdown().await;
}

/// The roster stops listing an identity, Core retires its member, and
/// the process is killed inside the host's retirement hook. The next start
/// hands the retired incarnation to its hook, once, cause `unlisted`; the
/// exit interview the unlisting queued before the hook stays queued once;
/// and nothing the killed process completed before the unlisting (the
/// seats' `after_create`) is told again.
///
/// RED before lifecycle effects were projected from Core's journal: the hook
/// was called once, from the roster pass, and never again, so a kill inside
/// it lost the call.
#[tokio::test(flavor = "multi_thread")]
async fn a_kill_inside_the_unlisted_hook_delivers_it_once_after_the_restart() {
    const NAME: &str = "a_kill_inside_the_unlisted_hook_delivers_it_once_after_the_restart";
    const MOB: &str = "lifecycle-effect-unlisted-hook";
    let alpha = member(ALPHA);
    let beta = member(BETA);
    if let Some(state) = child_state() {
        let customizer = Arc::new(RecordingCustomizer::default());
        let boot = boot(
            &state,
            MOB,
            vec![alpha.clone(), beta.clone()],
            Wiring {
                observer: Some(exiting_observer()),
                customizer: Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>),
                memory: Some(memory_injector(
                    &state.join("memory"),
                    Arc::new(RecordingTranscripts::default()),
                )),
                ..Wiring::default()
            },
        )
        .await;
        // Both seats are told before the roster changes.
        eventually("the seats' after_create", || {
            (customizer.told().len() >= 2).then_some(())
        })
        .await;
        std::fs::write(
            state.join("retired-generation"),
            boot.generation_of(&beta.identity).await.to_string(),
        )
        .expect("note the generation the unlisting retires");
        assert!(boot.roster.remove(&beta.identity));
        // The hook ends the process.
        let _ = boot.context.refresh_desired_topology_tracked().await;
        std::future::pending::<()>().await;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    run_child(NAME, temp.path()).await;
    let retired_generation: u64 = std::fs::read_to_string(temp.path().join("retired-generation"))
        .expect("the child noted the retired generation")
        .parse()
        .expect("a generation");

    let seen = Arc::new(Mutex::new(Vec::new()));
    let customizer = Arc::new(RecordingCustomizer::default());
    let (memory, memory_store) = memory_over(
        &temp.path().join("memory"),
        Arc::new(RecordingTranscripts::default()),
    );
    let boot = boot(
        temp.path(),
        MOB,
        vec![alpha.clone()],
        Wiring {
            observer: Some(recording_observer(&seen)),
            customizer: Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>),
            memory: Some(memory),
            ..Wiring::default()
        },
    )
    .await;
    eventually("the unlisted incarnation's hook", || {
        (!seen.lock().unwrap().is_empty()).then_some(())
    })
    .await;
    tokio::time::sleep(SETTLE).await;
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "the hook is called once: {calls:?}");
    assert_eq!(calls[0].identity, beta.identity);
    assert_eq!(calls[0].cause.as_str(), "unlisted");
    assert_eq!(calls[0].generation, retired_generation);
    assert_eq!(
        calls[0].registration, None,
        "the registration ended with the killed process, which this one never saw"
    );
    assert_eq!(
        pending_harvests_of(&memory_store, &beta.identity).await,
        vec!["retire".to_string()],
        "the exit interview is queued once, however often its step is replayed"
    );
    assert!(
        customizer.told().is_empty(),
        "nothing the killed process completed is told again: {:?}",
        customizer.told()
    );
    let _ = boot.runtime.shutdown().await;
}

/// The roster stops listing an identity, Core retires its member, and the
/// process is killed while the retired session is being distilled, before
/// the exit interview is queued. The next start queues it, once.
///
/// RED before lifecycle effects were projected from Core's journal: the exit
/// interview was queued inline by the roster pass right after the
/// distillation, so a kill before it lost it for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_kill_before_the_exit_interview_queues_it_after_the_restart() {
    const NAME: &str = "a_kill_before_the_exit_interview_queues_it_after_the_restart";
    const MOB: &str = "lifecycle-effect-exit-interview";
    let alpha = member(ALPHA);
    let beta = member(BETA);
    if let Some(state) = child_state() {
        let boot = boot(
            &state,
            MOB,
            vec![alpha.clone(), beta.clone()],
            Wiring {
                memory: Some(memory_injector(&state.join("memory"), Arc::new(ExitOnRead))),
                ..Wiring::default()
            },
        )
        .await;
        assert!(boot.roster.remove(&beta.identity));
        // The distillation of the retired session ends the process.
        let _ = boot.context.refresh_desired_topology_tracked().await;
        std::future::pending::<()>().await;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    run_child(NAME, temp.path()).await;
    {
        let store = SqliteAgentMemoryStore::open(temp.path().join("memory"))
            .expect("open the memory store");
        assert!(
            pending_harvests_of(&store, &beta.identity).await.is_empty(),
            "the kill came before the exit interview"
        );
    }

    let (memory, memory_store) = memory_over(
        &temp.path().join("memory"),
        Arc::new(RecordingTranscripts::default()),
    );
    let boot = boot(
        temp.path(),
        MOB,
        vec![alpha.clone()],
        Wiring {
            memory: Some(memory),
            ..Wiring::default()
        },
    )
    .await;
    let deadline = tokio::time::Instant::now() + EFFECT_DEADLINE;
    let harvests = loop {
        let harvests = pending_harvests_of(&memory_store, &beta.identity).await;
        if !harvests.is_empty() {
            break harvests;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the exit interview was not queued within {EFFECT_DEADLINE:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(harvests, vec!["retire".to_string()]);
    let _ = boot.runtime.shutdown().await;
}

/// A seat commits in Core, and the process is killed inside the customizer's
/// `after_create` for it. The next start tells `after_create` for that seat,
/// once, with the session the identity is bound to.
///
/// RED before lifecycle effects were projected from Core's journal: the
/// notice was owed in memory only (a task, or the roster pass's own list),
/// and the next start found the identity seated, so nothing told it.
#[tokio::test(flavor = "multi_thread")]
async fn a_kill_inside_after_create_tells_it_after_the_restart() {
    const NAME: &str = "a_kill_inside_after_create_tells_it_after_the_restart";
    const MOB: &str = "lifecycle-effect-after-create";
    let alpha = member(ALPHA);
    if let Some(state) = child_state() {
        // The seat's after_create ends the process, during the roster pass
        // or after it.
        let _boot = boot(
            &state,
            MOB,
            vec![alpha.clone()],
            Wiring {
                customizer: Some(Arc::new(ExitingAfterCreate)),
                ..Wiring::default()
            },
        )
        .await;
        std::future::pending::<()>().await;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    run_child(NAME, temp.path()).await;

    let customizer = Arc::new(RecordingCustomizer::default());
    let boot = boot(
        temp.path(),
        MOB,
        vec![alpha.clone()],
        Wiring {
            customizer: Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>),
            ..Wiring::default()
        },
    )
    .await;
    eventually("the seat's after_create", || {
        (!customizer.told().is_empty()).then_some(())
    })
    .await;
    tokio::time::sleep(SETTLE).await;
    let session = boot.session_of(&alpha.identity).await;
    assert_eq!(
        customizer.told(),
        vec![(alpha.identity.clone(), session)],
        "the seat is told once, with the identity's session"
    );
    let _ = boot.runtime.shutdown().await;
}

/// A seat commits in Core, and the process is killed inside the customizer's
/// `after_create` for it. The next start's roster no longer lists the
/// identity: its boot pass retires the incarnation the seat made, and the
/// replay of the seat tells nothing for it, since its retirement hook
/// (`unlisted`) stands for it. The identity the roster still lists is told,
/// once.
///
/// RED before only the incarnation Core still binds was told: the replayed
/// seat of the retired incarnation was told `after_create`, after its
/// retirement, with a session no member of the identity runs on.
#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_seat_of_an_incarnation_that_ended_is_not_told() {
    const NAME: &str = "a_replayed_seat_of_an_incarnation_that_ended_is_not_told";
    const MOB: &str = "lifecycle-effect-ended-seat";
    let alpha = member(ALPHA);
    let beta = member(BETA);
    if let Some(state) = child_state() {
        // A seat's after_create ends the process, during the roster pass or
        // after it.
        let _boot = boot(
            &state,
            MOB,
            vec![alpha.clone(), beta.clone()],
            Wiring {
                customizer: Some(Arc::new(ExitingAfterCreate)),
                ..Wiring::default()
            },
        )
        .await;
        std::future::pending::<()>().await;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    run_child(NAME, temp.path()).await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let customizer = Arc::new(RecordingCustomizer::default());
    let boot = boot(
        temp.path(),
        MOB,
        vec![alpha.clone()],
        Wiring {
            observer: Some(recording_observer(&seen)),
            customizer: Some(Arc::clone(&customizer) as Arc<dyn AgentCustomizer>),
            ..Wiring::default()
        },
    )
    .await;
    eventually("the ended incarnation's retirement hook", || {
        (!hook_calls_of(&seen, &beta.identity).is_empty()).then_some(())
    })
    .await;
    eventually("the listed seat's after_create", || {
        (!customizer.told().is_empty()).then_some(())
    })
    .await;
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        hook_calls_of(&seen, &beta.identity),
        vec!["unlisted".to_string()],
        "the retirement of the incarnation the seat made is handed over once"
    );
    let session = boot.session_of(&alpha.identity).await;
    assert_eq!(
        customizer.told(),
        vec![(alpha.identity.clone(), session)],
        "only the incarnation Core still binds is told"
    );
    let _ = boot.runtime.shutdown().await;
}

/// The host's retirement hook never answers for one identity the roster
/// stops listing. Each attempt to call it is bounded: once every attempt
/// missed its deadline, the failure is reported as a retirement hook's
/// failure is (`retired_member_release_failed`, its error saying the hook
/// timed out), and the retirement Core journaled after it reaches the hook.
///
/// RED before each lifecycle side effect was bounded: the projection waited
/// for the hook forever, and every side effect journaled after it waited
/// behind it.
#[tokio::test(flavor = "multi_thread")]
async fn a_retirement_hook_that_never_answers_is_reported_and_later_effects_run() {
    const MOB: &str = "lifecycle-effect-stuck-hook";
    let temp = tempfile::tempdir().expect("temp dir");
    let alpha = member(ALPHA);
    let beta = member(BETA);
    let gamma = member(GAMMA);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let boot = boot(
        temp.path(),
        MOB,
        vec![alpha.clone(), beta.clone(), gamma.clone()],
        Wiring {
            observer: Some(hook_stuck_on(&beta.identity, &seen)),
            error_hook: Some(keeping_error_hook(&events)),
            effect_limits: Some((SHORT_DEADLINE, SHORT_ATTEMPTS)),
            ..Wiring::default()
        },
    )
    .await;
    assert!(boot.roster.remove(&beta.identity));
    boot.context
        .refresh_desired_topology_tracked()
        .await
        .expect("the roster pass retires the unlisted identity");
    eventually("the first call of the hook that never answers", || {
        (!hook_calls_of(&seen, &beta.identity).is_empty()).then_some(())
    })
    .await;
    // A step Core journals after the one whose hook never answers.
    assert!(boot.roster.remove(&gamma.identity));
    boot.context
        .refresh_desired_topology_tracked()
        .await
        .expect("the roster pass retires the second unlisted identity");
    eventually("the later retirement's hook", || {
        (!hook_calls_of(&seen, &gamma.identity).is_empty()).then_some(())
    })
    .await;
    eventually("the reported failure of the hook", || {
        (!failed_releases_of(&events, &beta.identity).is_empty()).then_some(())
    })
    .await;
    tokio::time::sleep(SETTLE).await;
    let failed = failed_releases_of(&events, &beta.identity);
    assert_eq!(failed.len(), 1, "reported once: {failed:?}");
    assert_eq!(failed[0]["cause"], "unlisted", "{failed:?}");
    assert!(
        failed[0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("timed out")),
        "the error says the hook timed out: {failed:?}"
    );
    assert!(
        failed_effects_of(&events, &beta.identity).is_empty(),
        "a retirement hook's failure is reported once, as a failed release"
    );
    assert_eq!(
        hook_calls_of(&seen, &beta.identity).len(),
        SHORT_ATTEMPTS as usize,
        "each attempt calls the hook once, and no attempt follows the last"
    );
    assert_eq!(
        hook_calls_of(&seen, &gamma.identity),
        vec!["unlisted".to_string()]
    );
    let _ = boot.runtime.shutdown().await;
}

/// The exit-interview queue refuses every write. Each attempt to queue an
/// unlisted identity's exit interview is bounded: once every attempt failed,
/// the failure is reported typed (`lifecycle_effect_failed`, effect
/// `exit_interview`), the identity still reaches the retirement hook, and the
/// retirement Core journaled after it runs its side effects too.
///
/// RED before each lifecycle side effect was bounded: the failed write failed
/// the projection's page, which ran again from the same place at every
/// sweep, so neither the identity's hook nor anything journaled after it
/// ever ran.
#[tokio::test(flavor = "multi_thread")]
async fn an_exit_interview_write_that_always_fails_is_reported_and_later_effects_run() {
    const MOB: &str = "lifecycle-effect-refused-write";
    let temp = tempfile::tempdir().expect("temp dir");
    let alpha = member(ALPHA);
    let beta = member(BETA);
    let gamma = member(GAMMA);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (memory, memory_store) = memory_over(
        &temp.path().join("memory"),
        Arc::new(RecordingTranscripts::default()),
    );
    let boot = boot(
        temp.path(),
        MOB,
        vec![alpha.clone(), beta.clone(), gamma.clone()],
        Wiring {
            observer: Some(recording_observer(&seen)),
            memory: Some(memory),
            error_hook: Some(keeping_error_hook(&events)),
            effect_limits: Some((SHORT_DEADLINE, SHORT_ATTEMPTS)),
            ..Wiring::default()
        },
    )
    .await;
    refuse_exit_interviews(&memory_store).await;
    assert!(boot.roster.remove(&beta.identity));
    boot.context
        .refresh_desired_topology_tracked()
        .await
        .expect("the roster pass retires the unlisted identity");
    // A step Core journals after the one whose exit interview is refused.
    assert!(boot.roster.remove(&gamma.identity));
    boot.context
        .refresh_desired_topology_tracked()
        .await
        .expect("the roster pass retires the second unlisted identity");
    eventually("the later retirement's hook", || {
        (!hook_calls_of(&seen, &gamma.identity).is_empty()).then_some(())
    })
    .await;
    eventually("the reported failure of the exit interview", || {
        (!failed_effects_of(&events, &beta.identity).is_empty()).then_some(())
    })
    .await;
    tokio::time::sleep(SETTLE).await;
    let failed = failed_effects_of(&events, &beta.identity);
    assert_eq!(failed.len(), 1, "reported once: {failed:?}");
    assert_eq!(failed[0]["effect"], "exit_interview", "{failed:?}");
    assert_eq!(failed[0]["attempts"], SHORT_ATTEMPTS, "{failed:?}");
    assert_eq!(
        hook_calls_of(&seen, &beta.identity),
        vec!["unlisted".to_string()],
        "the refused exit interview holds back none of the identity's other side effects"
    );
    assert!(
        pending_harvests_of(&memory_store, &beta.identity)
            .await
            .is_empty(),
        "the refused write queued nothing"
    );
    let _ = boot.runtime.shutdown().await;
}
