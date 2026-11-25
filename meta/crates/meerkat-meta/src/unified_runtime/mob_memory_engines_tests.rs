//! Rows of the memory engines (the distiller and the steward) of a mob a
//! host serves beside its launch mob.
//!
//! The fixture is the launch of the memory rows on a launch whose memory
//! runs both engines: the launch mob's memory is assembled with them, and
//! the launch hands over, with its memory, what the engines of the mobs it
//! hosts are built with. Every engine of the rows asks one scripted model,
//! which answers a phase that wants one JSON object with one text and a
//! phase that wants one JSON array with another, and keeps what it was
//! asked. Each mob has an agent of ONE name, `desk`.

#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use meerkat_mob::{MobDefinition, MobId};

use super::mob_entries_tests::{
    configured_agent, definition, identity_context, launch_composed, second_mob, set_of, shut_down,
};
use super::mob_memory_firewall_tests::a_session;
use super::mob_memory_tests::{
    DESK, DeskRoster, KeptMemory, LaunchWithMemory, built_desk, call_memory_tool,
    config_of_the_launch, configured_desk, definition_keeping_memory,
    launch_with_memory_not_handed_over, memory_events_in, memory_events_once, with_a_second_mob,
};
use super::{StoredHost, UnifiedRuntime, asleep, known};
use crate::gateway_wiring::{HostMobEngines, HostMobIdentities, HostMobMemory};
use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryLlmWrites, AgentMemoryProvider,
};
use crate::identity_first::{
    AgentCustomizer, AgentMemoryCustomizer, AgentMemoryRuntimeInjector, AgentRuntimeServices,
    DurabilityPolicy, DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, RosterContext, RosterError,
    RosterProvider,
};
use crate::memory::capabilities::{MemoryPanelStore, PersistedDreamRun, StewardStore};
use crate::memory::coordinator::{MobScopeResolver, StaticMobBinding};
use crate::memory::distiller::{DistillerClientHandle, DistillerError, TranscriptSource};
use crate::memory::records::{ManifestTier, MemoryScope};
use crate::memory::sqlite_store::{HOSTED_MOBS_DIR, SqliteAgentMemoryStore};
use crate::memory::steward::{
    DreamOutcome, StewardClientHandle, StewardConfig, StewardEngine, StewardError,
};
use crate::memory_wiring::{
    EngineClientsInTests, HostedMobDream, MemoryEnginesConfig, MemoryStackOver, MemoryStackSeams,
};
use crate::mob_handle_runtime::MobBootstrapSpec;

/// The one model of the rows' engines. A phase whose prompt asks for one
/// JSON array is answered with the array text, every other phase with the
/// object text; a row sets either before the dream it reads. It keeps the
/// prompts it was sent.
#[derive(Clone)]
pub(super) struct ScriptedEngines {
    object: Arc<Mutex<String>>,
    array: Arc<Mutex<String>>,
    asked: Arc<Mutex<Vec<String>>>,
    /// `true` while the model holds every answer back: a prompt is kept as
    /// asked at once, and answered once the hold is let go.
    held: Arc<tokio::sync::watch::Sender<bool>>,
}

impl ScriptedEngines {
    pub(super) fn new() -> Self {
        Self {
            object: Arc::new(Mutex::new("{}".to_string())),
            array: Arc::new(Mutex::new("[]".to_string())),
            asked: Arc::default(),
            held: Arc::new(tokio::sync::watch::Sender::new(false)),
        }
    }

    /// The model holds every answer back from now on: an engine that asks
    /// it stays at its model call.
    pub(super) fn holds(&self) {
        self.held.send_replace(true);
    }

    /// The model answers again, the calls it held included.
    pub(super) fn releases(&self) {
        self.held.send_replace(false);
    }

    /// The model is asked a prompt it had not been asked when it had been
    /// asked `before` prompts, within ten seconds: an engine is at its
    /// model call.
    pub(super) async fn is_asked_after(&self, before: usize) -> bool {
        for _ in 0..200 {
            if self.asked().len() > before {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    /// Answer every phase that wants one JSON object with `value`.
    pub(super) fn answers_objects_with(&self, value: &serde_json::Value) {
        *self.object.lock().unwrap_or_else(PoisonError::into_inner) = value.to_string();
    }

    /// Answer every phase that wants one JSON array with `value`.
    pub(super) fn answers_arrays_with(&self, value: &serde_json::Value) {
        *self.array.lock().unwrap_or_else(PoisonError::into_inner) = value.to_string();
    }

    /// Every prompt the model was sent.
    pub(super) fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// This model as the client of both engines.
    pub(super) fn clients(&self) -> EngineClientsInTests {
        EngineClientsInTests {
            distiller: Arc::new(self.clone()),
            steward: Arc::new(self.clone()),
        }
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for ScriptedEngines {
    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let prompt = request
            .messages
            .iter()
            .map(|message| match message {
                meerkat_core::Message::User(user) => user.text_content(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let wants_an_array = prompt.to_lowercase().contains("exactly one json array");
        let reply = if wants_an_array {
            &self.array
        } else {
            &self.object
        }
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(prompt);
        use futures::StreamExt as _;
        let mut events: Vec<Result<meerkat_client::LlmEvent, meerkat_client::LlmError>> = vec![
            Ok(meerkat_client::LlmEvent::TextDelta {
                delta: reply,
                meta: None,
            }),
            Ok(meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            }),
        ];
        // The answer comes once the model does not hold it back.
        let mut held = self.held.subscribe();
        Box::pin(
            futures::stream::once(async move {
                let _ = held.wait_for(|held| !*held).await;
            })
            .flat_map(move |()| futures::stream::iter(std::mem::take(&mut events))),
        )
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl StewardClientHandle for ScriptedEngines {
    async fn client(&self) -> Result<Arc<dyn meerkat_client::LlmClient>, StewardError> {
        Ok(Arc::new(self.clone()))
    }

    fn invalidate(&self) {}
}

#[async_trait::async_trait]
impl DistillerClientHandle for ScriptedEngines {
    async fn client(&self) -> Result<Arc<dyn meerkat_client::LlmClient>, DistillerError> {
        Ok(Arc::new(self.clone()))
    }

    fn invalidate(&self) {}
}

/// A launch whose memory runs both engines, with a second mob of the host.
struct Engined {
    kept: KeptMemory,
    /// The model of every engine of the launch.
    scripted: ScriptedEngines,
    /// The session store the engines of every mob of the launch read.
    sessions: Arc<meerkat::MemoryStore>,
    /// The gating observers the host had before its second mob was made.
    observers_of_the_launch: usize,
}

/// Both engines on. A dream runs on one signal, and often enough for a row.
pub(super) fn engines_on() -> MemoryEnginesConfig {
    MemoryEnginesConfig {
        distiller: crate::memory::distiller::DistillerConfig {
            enabled: true,
            ..Default::default()
        },
        steward: StewardConfig {
            enabled: true,
            min_signals: 1,
            runs_per_day: 16,
            ..StewardConfig::default()
        },
    }
}

/// A launch that keeps agent memory under `temp` with the configuration
/// `config` and runs both engines for it, as a gateway assembles its launch
/// mob's memory, and that hands its memory over with what the engines of
/// the mobs it hosts are built with; then a second mob of the host whose
/// roster holds an agent `desk` too.
async fn launch_with_engines(temp: &tempfile::TempDir, config: AgentMemoryConfig) -> Engined {
    let scripted = ScriptedEngines::new();
    let sessions = Arc::new(meerkat::MemoryStore::new());
    let engines = engines_on();
    let (mut host, _sessions, _machine, ()) = launch_composed(
        temp,
        definition_keeping_memory("host"),
        meerkat::Config::default(),
        |_| (),
        |spec| spec,
    )
    .await;
    let launch_mob = host.mob_handle().mob_id().clone();
    let of_the_launch = SqliteAgentMemoryStore::open(temp.path().join("agent-memory"))
        .expect("the launch's memory store");
    let stack = crate::memory_wiring::assemble_memory_stack(
        Arc::new(of_the_launch.clone()),
        &config,
        &engines,
        MemoryStackSeams {
            persistent_state: Some(temp.path().to_path_buf()),
            transcript_store: Some(Arc::clone(&sessions) as Arc<dyn meerkat::SessionStore>),
            event_sink: Some(host.memory_event_sink()),
            ..Default::default()
        },
        None,
        MemoryStackOver {
            clients: Some(scripted.clients()),
            ..Default::default()
        },
    )
    .expect("the launch's memory stack");
    let resolver: Arc<dyn MobScopeResolver> = Arc::new(StaticMobBinding {
        realm: config.realm.clone(),
        mob: launch_mob.to_string(),
    });
    let customizer: Arc<dyn AgentCustomizer> = Arc::new(
        AgentMemoryCustomizer::wrap(None, stack.provider.clone(), config.clone())
            .with_mob_resolver(Some(Arc::clone(&resolver))),
    );
    let mut injector = AgentMemoryRuntimeInjector::new(stack.provider.clone(), config.clone())
        .with_taint_tracker(stack.taint.clone())
        .with_mob_resolver(Some(resolver));
    if let Some(distiller) = stack.distiller.clone() {
        injector = injector.with_distiller(distiller);
    }
    if let Some(steward) = stack.steward.clone() {
        injector = injector.with_steward(steward);
    }

    let handle = host.mob_runtime().handle().observer();
    let identity_runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "mob-memory-engines-launch".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(crate::gateway_wiring::identity_session_bridge(
                host.mob_runtime(),
                handle.clone(),
            ))),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(handle.clone())),
    );
    identity_runtime.set_agent_memory(Some(injector)).await;
    identity_runtime
        .set_agent_customizer(Some(Arc::clone(&customizer)))
        .await;
    let roster = Arc::new(DeskRoster::default());
    roster.seat_desk_in(&launch_mob);
    let launch_context = Arc::new(IdentityFirstRuntimeContext::new(
        identity_runtime,
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        Some(customizer),
        Some(handle.definition().clone()),
    ));
    Box::pin(host.install_and_bootstrap_identity_first_context(
        Arc::clone(&launch_context),
        &[configured_desk()],
    ))
    .await
    .expect("the launch mob's identity bootstrap");

    // The launch hands over its memory and what the engines of its other
    // mobs are built with: the engines it runs, its state directory and the
    // session store its mobs persist to.
    HostMobIdentities::new(Arc::clone(&roster) as Arc<dyn RosterProvider>, true)
        .with_agent_memory_per_mob(Some(
            HostMobMemory::new(of_the_launch.clone(), config.clone(), launch_mob).with_engines(
                Some(
                    HostMobEngines::new(
                        engines,
                        temp.path().to_path_buf(),
                        Arc::clone(&sessions) as Arc<dyn meerkat::SessionStore>,
                    )
                    .with_clients_in_tests(scripted.clients()),
                ),
            ),
        ))
        .hand_over_to(&host);
    let observers_of_the_launch = host.gating_resolution_observers_in_tests().await;
    let kept = with_a_second_mob(LaunchWithMemory {
        host,
        roster,
        of_the_launch,
        config,
        launch_context,
        taint_of_the_launch: stack.taint,
        provider_of_the_launch: stack.provider,
    })
    .await;
    let engined = Engined {
        kept,
        scripted,
        sessions,
        observers_of_the_launch,
    };
    engined.the_second_is_loaded().await;
    engined
}

impl Engined {
    /// The second mob's identity plane is composed, and the ask of the
    /// dream the mob is owed at this load ran to its end: the record of
    /// the launch's mobs has read the mob, and no dream of the load meets
    /// what a row seeds afterwards.
    async fn the_second_is_loaded(&self) {
        let second = &self.kept.second_id;
        let composed = self.kept.second_context().await;
        assert!(composed.is_some(), "the second mob has an identity plane");
        let mut asked = false;
        for _ in 0..400 {
            self.the_record_reads_the_mobs().await;
            asked = self
                .kept
                .host
                .mob_entry_in_tests(second)
                .is_some_and(|entry| entry.owed_dream_is_asked_in_tests());
            if asked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            asked,
            "the dream the second mob is owed at its load was asked"
        );
    }

    /// The dream of the second mob, as the host's dream occurrence reaches
    /// it: none while that mob has no steward.
    fn dream_of_the_second(&self) -> Option<HostedMobDream> {
        (self.kept.host.hosted_mob_dreams())()
            .into_iter()
            .find(|dream| dream.mob == self.kept.second_id)
    }

    /// The record of the launch's mobs reads the mobs the set runs: it
    /// then knows who of each is private to a person, and when each mob's
    /// log began.
    async fn the_record_reads_the_mobs(&self) {
        let read = self
            .kept
            .host
            .mob_runtime()
            .private_owners()
            .read_loaded_mobs()
            .await;
        assert!(read.is_ok(), "the record reads the loaded mobs: {read:?}");
    }

    /// The shared memory scope of the mob `mob` in the realm `realm`.
    fn shared_scope(realm: &str, mob: &MobId) -> MemoryScope {
        MemoryScope::Mob {
            realm: realm.to_string(),
            mob: mob.to_string(),
        }
    }
}

/// The titles of the active records of `scope` in `store`, sorted.
pub(super) async fn titles_in(store: &SqliteAgentMemoryStore, scope: &MemoryScope) -> Vec<String> {
    let mut titles: Vec<String> = store
        .manifest(std::slice::from_ref(scope), ManifestTier::Full)
        .await
        .expect("the scope's manifest")
        .into_iter()
        .map(|meta| meta.title)
        .collect();
    titles.sort();
    titles
}

/// The agent `desk` built from `context` over `handle` calls its `memory`
/// tool with `args`.
async fn desk_calls_memory(
    context: &IdentityFirstRuntimeContext,
    handle: meerkat_mob::MobHandle,
    args: serde_json::Value,
) {
    let draft = built_desk(context, handle).await;
    let (answer, failed) = call_memory_tool(&draft, args).await;
    assert!(!failed, "the agent's `memory` call is answered: {answer}");
}

/// A launch whose distiller and steward are on gives each mob it hosts
/// beside its launch mob engines of that mob's own: the second mob's memory
/// delivers with a distiller and a steward, and the host's dream occurrence
/// reaches that mob's dream. Composing them registers no gating observer
/// with the host, and the launch's memory root still lists the launch mob's
/// realm alone.
#[tokio::test]
async fn a_launch_whose_engines_are_on_gives_each_hosted_mob_engines_of_its_own() {
    let temp = tempfile::tempdir().expect("tempdir");
    let engined = launch_with_engines(&temp, config_of_the_launch()).await;
    let kept = &engined.kept;

    let second_context = kept
        .second_context()
        .await
        .expect("the second mob has an identity plane");
    let of_the_second = second_context
        .runtime
        .agent_memory_in_tests()
        .await
        .map(|memory| memory.engines_in_tests());
    assert_eq!(
        of_the_second,
        Some((true, true)),
        "the second mob's memory delivers with a distiller and a steward of its own"
    );
    assert!(
        engined.dream_of_the_second().is_some(),
        "the host's dream occurrence reaches the second mob's dream"
    );
    assert_eq!(
        kept.host.gating_resolution_observers_in_tests().await,
        engined.observers_of_the_launch,
        "composing the second mob's engines registered no gating observer with the host"
    );
    assert_eq!(
        kept.of_the_launch
            .known_realms()
            .expect("the launch's realms"),
        std::slice::from_ref(&kept.config.realm),
        "the launch's memory root lists the launch mob's realm alone"
    );
    engined.kept.shut_down().await;
}

const PROPOSED_IN_THE_LAUNCH_MOB: &str = "The launch mob closes its books on Mondays";
const PROPOSED_IN_THE_SECOND_MOB: &str = "The second mob closes its books on Fridays";

/// A dream of one mob reads and writes its own mob's store alone. An agent
/// of each mob proposed a record to its mob. The second mob's dream is
/// shown the second mob's proposal and not the launch mob's, accepts it,
/// and the record is then shared memory of the second mob, in the second
/// mob's store. The launch mob's proposal still waits in the launch's
/// store, and the launch mob's shared memory holds nothing.
#[tokio::test]
async fn a_dream_of_one_mob_reads_and_writes_its_own_store_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let engined = launch_with_engines(&temp, config_of_the_launch()).await;
    let kept = &engined.kept;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let realm_of_the_second = kept.realm_of_the_second();
    let of_the_second = kept.of_the_second();

    desk_calls_memory(
        &kept.launch_context,
        kept.host.mob_runtime().handle().observer(),
        serde_json::json!({
            "action": "propose_to_mob",
            "title": PROPOSED_IN_THE_LAUNCH_MOB,
            "body": "The launch mob's books are closed on Mondays.",
        }),
    )
    .await;
    let second_context = kept
        .second_context()
        .await
        .expect("the second mob has an identity plane");
    desk_calls_memory(
        &second_context,
        kept.second_handle().await,
        serde_json::json!({
            "action": "propose_to_mob",
            "title": PROPOSED_IN_THE_SECOND_MOB,
            "body": "The second mob's books are closed on Fridays.",
        }),
    )
    .await;
    let filed = of_the_second
        .pending_proposals(&realm_of_the_second, 16)
        .await
        .expect("the second mob's proposals");
    assert_eq!(
        filed.len(),
        1,
        "the second mob's proposal is filed: {filed:?}"
    );
    engined.scripted.answers_objects_with(&serde_json::json!({
        "proposal_verdicts": [{
            "proposal_id": filed[0].proposal_id.clone(),
            "verdict": "accept",
            "rationale": "knowledge of the mob's own",
        }],
    }));
    engined.the_record_reads_the_mobs().await;

    let dream = engined
        .dream_of_the_second()
        .expect("the second mob has a steward of its own");
    let outcome = dream.steward.dream_now().await;
    assert!(
        matches!(outcome, DreamOutcome::Completed(_)),
        "the second mob's dream runs: {outcome:?}"
    );

    assert_eq!(
        titles_in(
            &of_the_second,
            &Engined::shared_scope(&realm_of_the_second, &kept.second_id)
        )
        .await,
        [PROPOSED_IN_THE_SECOND_MOB],
        "the accepted record is shared memory of the second mob, in the second mob's store"
    );
    assert!(
        of_the_second
            .pending_proposals(&realm_of_the_second, 16)
            .await
            .expect("the second mob's proposals")
            .is_empty(),
        "and the second mob's proposal is settled"
    );
    let asked = engined.scripted.asked();
    assert!(
        asked
            .iter()
            .any(|prompt| prompt.contains(PROPOSED_IN_THE_SECOND_MOB)),
        "the dream was shown the second mob's proposal"
    );
    assert!(
        asked
            .iter()
            .all(|prompt| !prompt.contains(PROPOSED_IN_THE_LAUNCH_MOB)),
        "and nothing of the launch mob's"
    );
    assert_eq!(
        kept.of_the_launch
            .pending_proposals(&kept.config.realm, 16)
            .await
            .expect("the launch's proposals")
            .len(),
        1,
        "the launch mob's proposal still waits in the launch's store"
    );
    assert!(
        titles_in(
            &kept.of_the_launch,
            &Engined::shared_scope(&kept.config.realm, &launch_mob)
        )
        .await
        .is_empty(),
        "and the launch mob's shared memory holds nothing"
    );
    engined.kept.shut_down().await;
}

const NOTED_BY_THE_SECOND_DESK: &str = "The second desk files returns by post";

/// A retired agent of a hosted mob is harvested into its own mob's shared
/// memory. The second mob's agent has a note; its retirement queues its
/// exit interview in the second mob's store; the second mob's next dream
/// promotes the note into the second mob's shared memory and settles the
/// interview. The launch's store queues no interview and gains no record.
#[tokio::test]
async fn a_retired_agent_of_a_hosted_mob_is_harvested_into_its_own_mobs_shared_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let engined = launch_with_engines(&temp, config_of_the_launch()).await;
    let kept = &engined.kept;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let realm_of_the_second = kept.realm_of_the_second();
    let of_the_second = kept.of_the_second();
    let second_context = kept
        .second_context()
        .await
        .expect("the second mob has an identity plane");
    desk_calls_memory(
        &second_context,
        kept.second_handle().await,
        serde_json::json!({
            "action": "remember",
            "title": NOTED_BY_THE_SECOND_DESK,
            "body": "Returns of the second desk go out by post.",
        }),
    )
    .await;
    let desk_of_the_second = MemoryScope::Identity {
        realm: realm_of_the_second.clone(),
        identity: DESK.to_string(),
    };
    let noted = of_the_second
        .manifest(
            std::slice::from_ref(&desk_of_the_second),
            ManifestTier::Full,
        )
        .await
        .expect("the second desk's notes");
    assert_eq!(noted.len(), 1, "the second desk has its note: {noted:?}");

    // The retirement's exit interview, as the mob's lifecycle asks it of
    // the mob's own memory.
    let memory_of_the_second = second_context
        .runtime
        .agent_memory_in_tests()
        .await
        .expect("the second mob keeps agent memory");
    let queued = memory_of_the_second
        .record_identity_retired_at(
            &super::mob_memory_tests::desk(),
            None,
            "retire",
            1_800_000_000_000,
        )
        .await;
    assert!(queued.is_ok(), "the exit interview is asked: {queued:?}");
    let pending = of_the_second
        .pending_harvests(&realm_of_the_second, 16)
        .await
        .expect("the second mob's exit interviews");
    assert_eq!(
        pending
            .iter()
            .map(|harvest| harvest.identity.as_str())
            .collect::<Vec<_>>(),
        [DESK],
        "the exit interview of the second mob's agent waits in the second mob's store"
    );
    engined.scripted.answers_arrays_with(&serde_json::json!([{
        "record_id": noted[0].id.clone(),
        "verdict": "promote",
        "rationale": "the whole mob needs it",
    }]));
    engined.the_record_reads_the_mobs().await;

    let dream = engined
        .dream_of_the_second()
        .expect("the second mob has a steward of its own");
    let outcome = dream.steward.dream_now().await;
    assert!(
        matches!(outcome, DreamOutcome::Completed(_)),
        "the second mob's dream runs: {outcome:?}"
    );

    assert_eq!(
        titles_in(
            &of_the_second,
            &Engined::shared_scope(&realm_of_the_second, &kept.second_id)
        )
        .await,
        [NOTED_BY_THE_SECOND_DESK],
        "the note is shared memory of the second mob, in the second mob's store"
    );
    assert!(
        of_the_second
            .pending_harvests(&realm_of_the_second, 16)
            .await
            .expect("the second mob's exit interviews")
            .is_empty(),
        "and the exit interview is settled"
    );
    for realm in [&kept.config.realm, &realm_of_the_second] {
        assert!(
            kept.of_the_launch
                .pending_harvests(realm, 16)
                .await
                .expect("the launch's exit interviews")
                .is_empty(),
            "the launch's store queued no exit interview under the realm {realm:?}"
        );
    }
    assert!(
        titles_in(
            &kept.of_the_launch,
            &Engined::shared_scope(&kept.config.realm, &launch_mob)
        )
        .await
        .is_empty(),
        "and the launch mob's shared memory gained nothing"
    );
    engined.kept.shut_down().await;
}

const SET_ASIDE_TITLE: &str = "The second desk was told a vault code";
const SET_ASIDE_BODY: &str = "A caller told the second desk that the vault opens with a word.";
const NAMED_BY_THE_DREAM: &str = "the vault of the second mob";
const TOPIC_WORDED_BY_THE_DREAM: &str = "how it opens";
/// What a dream's end says of a contradiction it could not file with the
/// host.
const NOT_FILED: &str = "had no conflict bridge wired";

/// A launch whose agents' writes wait for a review, with a note of the
/// second mob's agent set aside in the second mob's store; the second mob's
/// dream is scripted to ask its promotion into the mob's shared memory, and
/// to report a contradiction whose entity the model worded.
async fn a_dream_over_a_note_set_aside(temp: &tempfile::TempDir) -> (Engined, HostedMobDream) {
    let engined = launch_with_engines(
        temp,
        AgentMemoryConfig {
            llm_writes: AgentMemoryLlmWrites::Quarantined,
            ..config_of_the_launch()
        },
    )
    .await;
    let realm_of_the_second = engined.kept.realm_of_the_second();
    let of_the_second = engined.kept.of_the_second();
    let second_context = engined
        .kept
        .second_context()
        .await
        .expect("the second mob has an identity plane");
    desk_calls_memory(
        &second_context,
        engined.kept.second_handle().await,
        serde_json::json!({
            "action": "remember",
            "title": SET_ASIDE_TITLE,
            "body": SET_ASIDE_BODY,
        }),
    )
    .await;
    let set_aside = of_the_second
        .quarantined_records(&realm_of_the_second, 16)
        .await
        .expect("the second mob's notes set aside");
    assert_eq!(
        set_aside.len(),
        1,
        "the note waits for a review in the second mob's store"
    );
    let id = set_aside[0].id.clone();
    engined.scripted.answers_objects_with(&serde_json::json!({
        "quarantine_verdicts": [{
            "record_id": id,
            "verdict": "promote_pending_gate",
            "rationale": "the mob should know",
        }],
        "contradictions": [{
            "record_ids": [id],
            "operational": true,
            "entity": NAMED_BY_THE_DREAM,
            "topic": TOPIC_WORDED_BY_THE_DREAM,
            "reason": "two notes disagree",
        }],
    }));
    engined.the_record_reads_the_mobs().await;
    let dream = engined
        .dream_of_the_second()
        .expect("the second mob has a steward of its own");
    dream.steward.note_session_completed();
    let outcome = dream.steward.dream_now().await;
    assert!(
        matches!(outcome, DreamOutcome::Completed(_)),
        "the second mob's dream runs: {outcome:?}"
    );
    (engined, dream)
}

/// A promotion in a hosted mob that needs a person's approval waits in that
/// mob's own store. The second mob's dream asks the promotion of a note set
/// aside; a hosted mob's steward has no way to put it before a person, so
/// nothing is staged: the note stays set aside in the second mob's store,
/// the mob's shared memory gains nothing, the host's approvals hold no
/// request of it and the host has no more gating observers than before.
#[tokio::test]
async fn a_promotion_of_a_hosted_mob_that_needs_a_person_waits_in_its_own_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (engined, _dream) = a_dream_over_a_note_set_aside(&temp).await;
    let kept = &engined.kept;
    let realm_of_the_second = kept.realm_of_the_second();
    let of_the_second = kept.of_the_second();

    assert_eq!(
        of_the_second
            .quarantined_records(&realm_of_the_second, 16)
            .await
            .expect("the second mob's notes set aside")
            .len(),
        1,
        "the note is still set aside in the second mob's store"
    );
    assert!(
        of_the_second
            .pending_promotions(&realm_of_the_second)
            .await
            .expect("the second mob's promotions")
            .is_empty(),
        "no promotion of it was staged"
    );
    assert!(
        titles_in(
            &of_the_second,
            &Engined::shared_scope(&realm_of_the_second, &kept.second_id)
        )
        .await
        .is_empty(),
        "the second mob's shared memory gained nothing"
    );
    let waiting = kept.host.list_gating_pending().await;
    assert!(
        waiting.is_empty(),
        "the host's approvals hold no request of the second mob's memory: {waiting:?}"
    );
    assert_eq!(
        kept.host.gating_resolution_observers_in_tests().await,
        engined.observers_of_the_launch,
        "and the host has the gating observers it had"
    );
    engined.kept.shut_down().await;
}

/// The events `events` holds, each as its type and its data's text, once
/// one of the type `awaited` whose data holds `holding` is among them or
/// after ten seconds: a memory event is filed by a task of its own, and a
/// mob's first load may have asked a dream of its own before the row's.
async fn events_once(
    events: &super::ConsoleEventStore,
    awaited: &str,
    holding: &str,
) -> Vec<(String, String)> {
    let mut filed = Vec::new();
    for _ in 0..100 {
        filed = events
            .replay_all(None)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|event| (event.event_type, event.data.to_string()))
            .collect::<Vec<_>>();
        if filed
            .iter()
            .any(|(event_type, data)| event_type == awaited && data.contains(holding))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    filed
}

/// The memory events of a hosted mob's engines are filed under that mob and
/// carry no record text. The second mob's dream files its start and its end
/// in the second mob's console events; the launch mob's console events hold
/// no dream event and nothing of the second mob. No event of either holds
/// the title or the body of the note the dream read, and none holds a word
/// the dream's model made from the records: the dream's end tells a
/// contradiction it could not file with the host by the count of its
/// records, and neither the entity nor the topic the model worded for it
/// is in an event of the second mob or of the launch mob.
#[tokio::test]
async fn the_memory_events_of_a_hosted_mobs_engines_are_filed_under_that_mob_and_carry_no_record_text()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let (engined, _dream) = a_dream_over_a_note_set_aside(&temp).await;
    let kept = &engined.kept;
    let of_the_host = kept.host.console_events();
    let of_the_second = of_the_host.of_mob(Some(&kept.second_id));

    let in_the_second = events_once(&of_the_second, "memory.dream.completed", NOT_FILED).await;
    let types: Vec<&str> = in_the_second
        .iter()
        .map(|(event_type, _)| event_type.as_str())
        .collect();
    assert!(
        types.contains(&"memory.dream.started") && types.contains(&"memory.dream.completed"),
        "the second mob's dream is filed in the second mob's console events: {types:?}"
    );
    let in_the_launch = events_once(&of_the_host, "memory.dream.completed", "").await;
    assert!(
        in_the_launch
            .iter()
            .all(|(event_type, _)| !event_type.starts_with("memory.dream.")),
        "the launch mob's console events hold no dream event: {in_the_launch:?}"
    );
    for (event_type, data) in in_the_second.iter().chain(in_the_launch.iter()) {
        assert!(
            !data.contains(SET_ASIDE_TITLE) && !data.contains(SET_ASIDE_BODY),
            "the event {event_type} holds neither the title nor the body of a record: {data}"
        );
    }
    assert!(
        in_the_second
            .iter()
            .any(|(event_type, data)| event_type == "memory.dream.completed"
                && data.contains(NOT_FILED)),
        "the dream's end says that a contradiction was not filed, in the second mob's events"
    );
    for (event_type, data) in in_the_second.iter().chain(in_the_launch.iter()) {
        assert!(
            !data.contains(NAMED_BY_THE_DREAM) && !data.contains(TOPIC_WORDED_BY_THE_DREAM),
            "the event {event_type} holds no word the dream's model made from the records: {data}"
        );
    }
    engined.kept.shut_down().await;
}

/// The session the mob of `handle` holds its agent `desk` bound to, as the
/// mob's own journal says it.
pub(super) async fn session_of_the_desk(
    handle: &meerkat_mob::MobHandle,
) -> meerkat_core::types::SessionId {
    handle
        .last_member_binding(&crate::member_comms_id::mob_member_id(DESK))
        .await
        .expect("the mob's journal reads")
        .and_then(|binding| binding.session_id().cloned())
        .expect("the mob's journal bound its desk to a session")
}

/// What a transcript source answers for one stored session: whether a read
/// holds it, and whether it is private to a person (`None` when the source
/// says that is unknown).
pub(super) async fn held_by(
    source: &Arc<dyn TranscriptSource>,
    session: &str,
) -> (bool, Option<bool>) {
    let read = source
        .read(session, 0)
        .await
        .expect("the source reads")
        .is_some();
    (read, source.private_to_a_person(session).await.ok())
}

/// A session the source holds, private to no one.
pub(super) const ITS_OWN: (bool, Option<bool>) = (true, Some(false));
/// A session the source does not hold: it reads as one the store does not
/// hold, and whether it is private to a person is unknown.
pub(super) const NOT_ITS_OWN: (bool, Option<bool>) = (false, None);

/// The memory engines of a hosted mob read the transcripts of that mob's
/// own sessions, and of no other mob's. Which session is a mob's own is
/// what the mob's journal bound. Asked of the source the second mob's
/// steward reads through: the session the journal holds the mob's desk
/// bound to is held; the session of the launch mob's desk, a session of no
/// member, and a session whose stored binding names the second mob while
/// the mob's journal never bound it (as a session of an earlier mob of the
/// id is) read as sessions the store does not hold. Whether such a session
/// is private to a person is unknown, never "private to no one".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_memory_engines_of_a_hosted_mob_read_the_transcripts_of_that_mobs_own_sessions_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let engined = launch_with_engines(&temp, config_of_the_launch()).await;
    let kept = &engined.kept;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let sessions = &engined.sessions;

    let of_the_second = a_session_under(
        sessions,
        Some(session_of_the_desk(&kept.second_handle().await).await),
        Some((&kept.second_id, DESK)),
        "said to the second mob's desk",
    )
    .await;
    let of_the_launch = a_session_under(
        sessions,
        Some(session_of_the_desk(&kept.host.mob_handle()).await),
        Some((&launch_mob, DESK)),
        "said to the launch mob's desk",
    )
    .await;
    let naming_the_second = a_session_of(
        sessions,
        &kept.second_id,
        DESK,
        "said to the desk of an earlier mob of the id",
    )
    .await;
    let of_no_member = a_session(sessions, None).await;
    engined.the_record_reads_the_mobs().await;

    let dream = engined
        .dream_of_the_second()
        .expect("the second mob has a steward of its own");
    let of_the_steward: Arc<dyn TranscriptSource> =
        Arc::clone(dream.steward.transcripts_in_tests());
    let table = [
        (
            &of_the_second,
            ITS_OWN,
            "the second mob's own, by its journal",
        ),
        (&of_the_launch, NOT_ITS_OWN, "the launch mob's"),
        (
            &naming_the_second,
            NOT_ITS_OWN,
            "one that names the second mob and that its journal never bound",
        ),
        (&of_no_member, NOT_ITS_OWN, "no member's"),
    ];
    for (session, expected, whose) in table {
        assert_eq!(
            held_by(&of_the_steward, session).await,
            expected,
            "a session that is {whose}"
        );
    }
    engined.kept.shut_down().await;
}

const DISTILLED_FOR_THE_SECOND_DESK: &str = "Parcels of the second desk leave at noon";

/// A session stored in `sessions` with one message `said`, whose stored
/// member binding names the member `bound_to` (or names none), private to
/// the person `private_to` (or to no one): its key. With `id` it is stored
/// under that id, the one a mob's journal bound, so the engines read a
/// transcript of a session that is the mob's own; without, under an id no
/// journal bound.
async fn a_stored_session(
    sessions: &Arc<meerkat::MemoryStore>,
    id: Option<meerkat_core::types::SessionId>,
    bound_to: Option<(&MobId, &str)>,
    private_to: Option<&str>,
    said: &str,
) -> String {
    let mut session = id.map_or_else(meerkat_core::Session::new, meerkat_core::Session::with_id);
    let stored = session.set_session_metadata(meerkat_core::SessionMetadata {
        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
        model: "gpt-5.5".to_string(),
        model_fallback: None,
        max_tokens: 1024,
        structured_output_retries: meerkat_core::config::default_structured_output_retries(),
        provider: meerkat_core::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        tooling: meerkat_core::SessionTooling::default(),
        keep_alive: false,
        comms_name: None,
        peer_meta: private_to.map(|person| meerkat_core::PeerMeta {
            description: None,
            labels: [(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                person.to_string(),
            )]
            .into_iter()
            .collect(),
        }),
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: None,
        mob_member_binding: bound_to.map(|(mob, member)| meerkat_core::MobMemberBinding {
            mob_id: mob.to_string(),
            role: "worker".to_string(),
            member: member.to_string(),
        }),
    });
    assert!(
        stored.is_ok(),
        "the session's metadata is recorded: {stored:?}"
    );
    session.push(meerkat_core::Message::User(
        meerkat_core::UserMessage::text(said),
    ));
    let saved = meerkat::SessionStore::save(&**sessions, &session).await;
    assert!(saved.is_ok(), "the session is stored: {:?}", saved.err());
    session.id().to_string()
}

/// [`a_stored_session`] private to no one.
pub(super) async fn a_session_under(
    sessions: &Arc<meerkat::MemoryStore>,
    id: Option<meerkat_core::types::SessionId>,
    bound_to: Option<(&MobId, &str)>,
    said: &str,
) -> String {
    a_stored_session(sessions, id, bound_to, None, said).await
}

/// [`a_session_under`] an id no journal bound, whose stored binding names
/// the member `member` of the mob `mob`.
pub(super) async fn a_session_of(
    sessions: &Arc<meerkat::MemoryStore>,
    mob: &MobId,
    member: &str,
    said: &str,
) -> String {
    a_session_under(sessions, None, Some((mob, member)), said).await
}

/// A run of a hosted mob's agent is distilled into its own identity memory.
/// A session of the second mob's agent ends; the second mob's distiller
/// reads it and writes what it learned under that agent's name, in the
/// second mob's store. The launch mob's agent of the same name gains
/// nothing.
#[tokio::test]
async fn a_run_of_a_hosted_mobs_agent_is_distilled_into_its_own_identity_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let engined = launch_with_engines(&temp, config_of_the_launch()).await;
    let kept = &engined.kept;
    let realm_of_the_second = kept.realm_of_the_second();
    let session = a_session_under(
        &engined.sessions,
        Some(session_of_the_desk(&kept.second_handle().await).await),
        Some((&kept.second_id, DESK)),
        "A carrier told the desk that its parcels leave at noon.",
    )
    .await;
    engined.the_record_reads_the_mobs().await;
    engined.scripted.answers_arrays_with(&serde_json::json!([{
        "action": "remember",
        "kind": "fact",
        "title": DISTILLED_FOR_THE_SECOND_DESK,
        "description": "When the second desk's parcels leave.",
        "body": "The parcels of the second desk leave at noon.",
        "tags": [],
        "epistemic": "observed",
        "evidence_range": [0, 0],
    }]));

    let second_context = kept
        .second_context()
        .await
        .expect("the second mob has an identity plane");
    let memory_of_the_second = second_context
        .runtime
        .agent_memory_in_tests()
        .await
        .expect("the second mob keeps agent memory");
    memory_of_the_second
        .distill_before_rotation(
            &super::mob_memory_tests::desk(),
            &session,
            crate::memory::distiller::DistillCause::Retire,
        )
        .await;

    assert_eq!(
        titles_in(
            &kept.of_the_second(),
            &MemoryScope::Identity {
                realm: realm_of_the_second,
                identity: DESK.to_string(),
            }
        )
        .await,
        [DISTILLED_FOR_THE_SECOND_DESK],
        "what the session taught is a note of the second mob's agent, in the second mob's store"
    );
    assert!(
        titles_in(
            &kept.of_the_launch,
            &MemoryScope::Identity {
                realm: kept.config.realm.clone(),
                identity: DESK.to_string(),
            }
        )
        .await
        .is_empty(),
        "the launch mob's agent of the same name gains nothing"
    );
    engined.kept.shut_down().await;
}

const HANDED_TO_THE_SECOND_DESK: &str = "Returns of the second desk close on Fridays";

/// A compaction harvest for a session the mob's journal never bound distils
/// nothing. The second mob's distiller is handed the discards of a
/// compaction for two sessions the mob's journal never bound: one whose
/// stored binding names the second mob (as a session of an earlier mob of
/// the id does), one of no member. Whether such a session is private to a
/// person is unknown, so neither is distilled: the second mob's agent gains
/// no note, and nothing is set aside for it.
#[tokio::test]
async fn a_compaction_harvest_for_a_session_the_mobs_journal_never_bound_distils_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let engined = launch_with_engines(&temp, config_of_the_launch()).await;
    let kept = &engined.kept;
    let realm_of_the_second = kept.realm_of_the_second();
    let naming_the_second = a_session_of(
        &engined.sessions,
        &kept.second_id,
        DESK,
        "said to the desk of an earlier mob of the id",
    )
    .await;
    let of_no_member = a_session(&engined.sessions, None).await;
    engined.the_record_reads_the_mobs().await;
    engined.scripted.answers_arrays_with(&serde_json::json!([{
        "action": "remember",
        "kind": "fact",
        "title": HANDED_TO_THE_SECOND_DESK,
        "description": "When the second desk's returns close.",
        "body": "The returns of the second desk close on Fridays.",
        "tags": [],
        "epistemic": "observed",
    }]));

    let second_context = kept
        .second_context()
        .await
        .expect("the second mob has an identity plane");
    let memory_of_the_second = second_context
        .runtime
        .agent_memory_in_tests()
        .await
        .expect("the second mob keeps agent memory");
    for session in [&naming_the_second, &of_no_member] {
        let harvested = memory_of_the_second
            .harvest_compaction_within_bound(
                &super::mob_memory_tests::desk(),
                session,
                Some(vec![crate::memory::distiller::DiscardEntry {
                    content: "The returns of the second desk close on Fridays.".to_string(),
                    range: None,
                }]),
                false,
                None,
            )
            .await;
        assert!(
            matches!(
                &harvested,
                Err(crate::identity_first::agent_memory::DistillerRunError::Failed(why))
                    if why.contains("is not one this mob's journal bound")
            ),
            "the harvest answers that it harvested nothing, and why: {harvested:?}"
        );
    }

    assert!(
        titles_in(
            &kept.of_the_second(),
            &MemoryScope::Identity {
                realm: realm_of_the_second.clone(),
                identity: DESK.to_string(),
            }
        )
        .await
        .is_empty(),
        "the second mob's agent gains no note from a session its mob never bound"
    );
    assert!(
        kept.of_the_second()
            .quarantined_records(&realm_of_the_second, 16)
            .await
            .expect("the second mob's notes set aside")
            .is_empty(),
        "and nothing is set aside for it"
    );
    engined.kept.shut_down().await;
}

/// The roster of a stored launch: the agent `desk`, for whichever mob it is
/// asked, so a launch after a restart seats the stored mobs' agents too.
pub(super) struct DeskEverywhere;

#[async_trait::async_trait]
impl RosterProvider for DeskEverywhere {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![configured_agent(DESK)])
    }
}

/// A host whose state directory outlives a launch, as the lazy start rows'
/// stored host, whose launches keep agent memory in a real directory, run
/// both engines for it and hand their memory over with what the engines of
/// the mobs they host are built with.
pub(super) struct StoredEngines {
    pub(super) stored: StoredHost,
    pub(super) scripted: ScriptedEngines,
    pub(super) sessions: Arc<meerkat::MemoryStore>,
}

/// One launch of a [`StoredEngines`] host.
pub(super) struct Launched {
    pub(super) host: UnifiedRuntime,
    /// The launch's own store, over the launch's memory root.
    pub(super) of_the_launch: SqliteAgentMemoryStore,
    /// The launch mob's steward.
    pub(super) steward: Arc<StewardEngine>,
}

impl StoredEngines {
    pub(super) fn new() -> Self {
        Self {
            stored: StoredHost::of(definition_keeping_memory("stored-host")),
            scripted: ScriptedEngines::new(),
            sessions: Arc::new(meerkat::MemoryStore::new()),
        }
    }

    /// The launch's memory root.
    pub(super) fn memory_root(&self) -> PathBuf {
        self.stored.state_dir().join("agent-memory")
    }

    /// A launch over the state directory, in a gateway's order: the spec
    /// declares the memory of the mobs the launch hosts, the store is
    /// opened and its release bound, the launch mob's memory is assembled
    /// with both engines, and the composer is handed over with the memory
    /// and what the other mobs' engines are built with.
    pub(super) async fn launch(&self) -> Launched {
        let mut host = self
            .stored
            .launch_composed(MobBootstrapSpec::with_hosted_mob_memory_declared)
            .await;
        let of_the_launch =
            SqliteAgentMemoryStore::open(self.memory_root()).expect("the launch's memory store");
        host.mob_runtime()
            .mob_memory_release()
            .bind(Arc::new(of_the_launch.clone()));
        let config = AgentMemoryConfig::default();
        let state = self.stored.state_dir().to_path_buf();
        let stack =
            crate::memory_wiring::assemble_memory_stack(
                Arc::new(of_the_launch.clone()),
                &config,
                &engines_on(),
                MemoryStackSeams {
                    persistent_state: Some(state.clone()),
                    transcript_store: Some(
                        Arc::clone(&self.sessions) as Arc<dyn meerkat::SessionStore>
                    ),
                    event_sink: Some(host.memory_event_sink()),
                    ..Default::default()
                },
                None,
                MemoryStackOver {
                    clients: Some(self.scripted.clients()),
                    ..Default::default()
                },
            )
            .expect("the launch's memory stack");
        let steward = stack.steward.clone().expect("the launch's steward");
        let of_the_launch_mob = identity_context(host.mob_runtime(), "default");
        of_the_launch_mob
            .runtime
            .set_agent_memory(Some(
                AgentMemoryRuntimeInjector::new(stack.provider.clone(), config.clone())
                    .with_steward(Arc::clone(&steward)),
            ))
            .await;
        Box::pin(host.install_and_bootstrap_identity_first_context(
            of_the_launch_mob,
            &[configured_agent(DESK)],
        ))
        .await
        .expect("the launch mob's identity bootstrap");
        let launch_mob = host.mob_handle().mob_id().clone();
        HostMobIdentities::new(Arc::new(DeskEverywhere) as Arc<dyn RosterProvider>, true)
            .with_agent_memory_per_mob(Some(
                HostMobMemory::new(of_the_launch.clone(), config, launch_mob).with_engines(Some(
                    HostMobEngines::new(
                        engines_on(),
                        state,
                        Arc::clone(&self.sessions) as Arc<dyn meerkat::SessionStore>,
                    )
                    .with_clients_in_tests(self.scripted.clients()),
                )),
            ))
            .hand_over_to(&host);
        Launched {
            host,
            of_the_launch,
            steward,
        }
    }
}

impl Launched {
    /// A store value over the own directory of the mob `mob`, for a row to
    /// seed and to read.
    pub(super) fn store_of(&self, mob: &MobId) -> SqliteAgentMemoryStore {
        self.of_the_launch
            .of_hosted_mob_in_tests(mob)
            .expect("the mob's own store")
    }

    /// The identity runtime of the mob `mob`. For a mob that is not loaded
    /// this is the request that loads it.
    pub(super) async fn runtime_of(&self, mob: &MobId) -> Arc<IdentityRuntime> {
        self.host
            .resolve_mob(Some(mob))
            .await
            .ok()
            .and_then(|mob| mob.identity_runtime_ref().cloned())
            .expect("the mob has an identity runtime")
    }

    /// A mob of the host beside its launch mob, created by the host from
    /// `definition`, with its identity plane composed, `desk` seated and
    /// what its first load asks answered ([`Self::loads`]).
    pub(super) async fn hosted_of(&self, definition: MobDefinition) -> MobId {
        let id = second_mob(&self.host, definition).await;
        self.loads(&id).await;
        id
    }

    /// [`Self::hosted_of`] a definition whose agents keep memory.
    pub(super) async fn hosted(&self, tag: &str) -> MobId {
        self.hosted_of(definition_keeping_memory(tag)).await
    }

    /// A request loads the mob `mob`, and the ask of the dream the mob is
    /// owed at this load is answered: the record of the launch's mobs has
    /// read the mob, and the ask ran to its end, with the dream it asked
    /// if it asked one. A row seeds a signal after it, so no dream of the
    /// load meets the signal.
    pub(super) async fn loads(&self, mob: &MobId) {
        let _ = self.runtime_of(mob).await;
        let mut asked = false;
        for _ in 0..400 {
            self.the_record_reads_the_mobs().await;
            asked = self
                .host
                .mob_entry_in_tests(mob)
                .is_some_and(|entry| entry.owed_dream_is_asked_in_tests());
            if asked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            asked,
            "the dream the mob {mob} is owed at its load was asked"
        );
    }

    /// Every dream run of the mob `mob` is moved two days into the past,
    /// through the store's own write of a run: whatever dream the mob had
    /// (its first load may have asked one) is then older than one dream
    /// cadence.
    pub(super) async fn ages_the_dreams_of(&self, mob: &MobId) {
        const TWO_DAYS_MS: u64 = 2 * 24 * 60 * 60 * 1000;
        let store = self.store_of(mob);
        let realm = realm_of(mob);
        let runs = MemoryPanelStore::dream_runs(&store, &realm, 32)
            .await
            .expect("the dream runs");
        for run in runs {
            let aged = store
                .save_dream_run(
                    &realm,
                    PersistedDreamRun {
                        started_at_ms: run.started_at_ms.saturating_sub(TWO_DAYS_MS),
                        completed_at_ms: run.completed_at_ms.saturating_sub(TWO_DAYS_MS),
                        ..run
                    },
                )
                .await;
            assert!(aged.is_ok(), "the dream run is moved back: {aged:?}");
        }
    }

    /// The agent `desk` of the mob `mob` remembers a note titled `title`:
    /// its id.
    pub(super) async fn remembers(&self, mob: &MobId, title: &str) -> String {
        let runtime = self.runtime_of(mob).await;
        let realm = runtime
            .agent_memory_realm()
            .await
            .expect("the mob keeps agent memory");
        runtime
            .remember_agent_memory(
                &realm,
                &super::mob_memory_tests::desk(),
                crate::identity_first::agent_memory::NewAgentMemory {
                    title: title.to_string(),
                    body: format!("the body of {title}"),
                    tags: Vec::new(),
                },
            )
            .await
            .expect("the agent's note is written")
            .memory_id
    }

    /// The mob set unloads its idle mobs, and the runtime lets go of the
    /// entry of each of `mobs`. A mob is unloaded only while it is idle,
    /// and one whose agent was just built may still be at work: the set is
    /// asked until each of `mobs` is not loaded, for twenty seconds.
    pub(super) async fn unloads(&self, mobs: &[&MobId]) {
        for _ in 0..200 {
            let _ = set_of(&self.host).unload_idle_mobs_for_tests().await;
            let known_now = known(&self.host).await;
            if mobs.iter().all(|mob| known_now.not_loaded.contains(*mob)) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        asleep(&self.host, mobs).await;
        for _ in 0..200 {
            let held = self.host.mob_entries_in_tests();
            if mobs.iter().all(|mob| !held.contains(*mob)) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let held = self.host.mob_entries_in_tests();
        assert!(
            mobs.iter().all(|mob| !held.contains(*mob)),
            "the entries of the unloaded mobs went: {held:?}"
        );
    }

    /// The record of the launch's mobs reads the mobs the set runs.
    pub(super) async fn the_record_reads_the_mobs(&self) {
        let read = self
            .host
            .mob_runtime()
            .private_owners()
            .read_loaded_mobs()
            .await;
        assert!(read.is_ok(), "the record reads the loaded mobs: {read:?}");
    }
}

/// The memory realm of the mob `mob`, a mob hosted beside the launch mob.
pub(super) fn realm_of(mob: &MobId) -> String {
    format!("mob.{mob}")
}

/// The dream runs that ended, in `realm` of `store`.
pub(super) async fn dreams_ended_in(store: &SqliteAgentMemoryStore, realm: &str) -> usize {
    MemoryPanelStore::dream_runs(store, realm, 32)
        .await
        .expect("the dream runs")
        .into_iter()
        .filter(|run| run.completed_at_ms > 0)
        .count()
}

/// The exit interview of a retired agent `gone`, who left no note, waits in
/// `realm` of `store`: a signal for the next dream of that store.
pub(super) async fn an_exit_interview_waits_in(store: &SqliteAgentMemoryStore, realm: &str) {
    let queued = store
        .record_pending_harvest_at(realm, "gone", None, "retire", 1_800_000_000_000)
        .await;
    assert!(queued.is_ok(), "the exit interview is queued: {queued:?}");
}

/// The exit interviews that wait in `realm` of `store`.
pub(super) async fn exit_interviews_waiting_in(
    store: &SqliteAgentMemoryStore,
    realm: &str,
) -> usize {
    store
        .pending_harvests(realm, 32)
        .await
        .expect("the exit interviews")
        .len()
}

/// The host's dream occurrence dreams for each loaded mob and loads none.
/// The launch mob and two mobs beside it each have a signal that waits for
/// a dream; the third mob is not loaded. One occurrence: the launch mob's
/// steward dreams over the launch's store, the second mob's over the second
/// mob's own; the third mob is still not loaded, and nothing dreamed over
/// its store, where its signal still waits.
#[tokio::test]
async fn the_dream_occurrence_dreams_for_each_loaded_mob_and_loads_none() {
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let second = launched.hosted("second").await;
    let third = launched.hosted("third").await;
    let launch_realm = AgentMemoryConfig::default().realm;
    for mob in [&second, &third] {
        launched.remembers(mob, "a note of this mob's desk").await;
    }
    an_exit_interview_waits_in(&launched.store_of(&third), &realm_of(&third)).await;
    launched.unloads(&[&second, &third]).await;
    // The second mob is loaded again, and what its load asks is answered
    // before its signal is there: the occurrence is what dreams over it.
    launched.loads(&second).await;
    an_exit_interview_waits_in(&launched.of_the_launch, &launch_realm).await;
    an_exit_interview_waits_in(&launched.store_of(&second), &realm_of(&second)).await;
    let of_the_launch_before = dreams_ended_in(&launched.of_the_launch, &launch_realm).await;
    let of_the_second_before =
        dreams_ended_in(&launched.store_of(&second), &realm_of(&second)).await;
    let of_the_third_before = dreams_ended_in(&launched.store_of(&third), &realm_of(&third)).await;

    crate::memory_wiring::dream_for_the_host(
        &launched.steward,
        Some(&launched.host.hosted_mob_dreams()),
    )
    .await;

    assert_eq!(
        dreams_ended_in(&launched.of_the_launch, &launch_realm).await,
        of_the_launch_before + 1,
        "the launch mob's steward dreamed over the launch's store"
    );
    assert_eq!(
        dreams_ended_in(&launched.store_of(&second), &realm_of(&second)).await,
        of_the_second_before + 1,
        "the second mob's steward dreamed over the second mob's own store"
    );
    assert_eq!(
        exit_interviews_waiting_in(&launched.store_of(&second), &realm_of(&second)).await,
        0,
        "and settled what waited there"
    );
    let known_now = known(&launched.host).await;
    assert!(
        known_now.not_loaded.contains(&third) && !known_now.loaded.contains(&third),
        "the occurrence loaded no mob: the set manages {:?}",
        known_now.loaded
    );
    assert_eq!(
        dreams_ended_in(&launched.store_of(&third), &realm_of(&third)).await,
        of_the_third_before,
        "nothing dreamed over the store of the mob that is not loaded"
    );
    assert_eq!(
        exit_interviews_waiting_in(&launched.store_of(&third), &realm_of(&third)).await,
        1,
        "where its signal still waits"
    );
    shut_down(&launched.host, &[&second]).await;
}

/// A dream a mob missed while it was not loaded is asked at its load. The
/// second mob has a signal that waits for a dream and is unloaded; no
/// occurrence reaches it, and whatever dream it had is older than one dream
/// cadence. The request that loads it asks one dream of its own steward,
/// which settles the signal. A second load inside the dream cadence asks
/// none, though a new signal waits.
#[tokio::test]
async fn a_dream_a_mob_missed_while_it_was_not_loaded_is_asked_at_its_load() {
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let second = launched.hosted("second").await;
    let of_the_second = launched.store_of(&second);
    let realm = realm_of(&second);
    launched
        .remembers(&second, "a note of the second desk")
        .await;
    an_exit_interview_waits_in(&of_the_second, &realm).await;
    launched.unloads(&[&second]).await;
    launched.ages_the_dreams_of(&second).await;
    let before = dreams_ended_in(&of_the_second, &realm).await;
    assert_eq!(
        exit_interviews_waiting_in(&of_the_second, &realm).await,
        1,
        "the signal waits while the mob is not loaded"
    );

    launched.loads(&second).await;
    assert_eq!(
        dreams_ended_in(&of_the_second, &realm).await,
        before + 1,
        "the mob's load asked the dream it missed"
    );
    assert_eq!(
        exit_interviews_waiting_in(&of_the_second, &realm).await,
        0,
        "and it settled what waited"
    );

    let queued = of_the_second
        .record_pending_harvest_at(&realm, "gone-later", None, "retire", 1_800_000_000_001)
        .await;
    assert!(queued.is_ok(), "a new signal waits: {queued:?}");
    launched.unloads(&[&second]).await;
    launched.loads(&second).await;
    assert_eq!(
        dreams_ended_in(&of_the_second, &realm).await,
        before + 1,
        "a load inside the dream cadence asks no dream"
    );
    assert_eq!(
        exit_interviews_waiting_in(&of_the_second, &realm).await,
        1,
        "the new signal waits for the next occurrence"
    );
    shut_down(&launched.host, &[&second]).await;
}

const NOTED_BEFORE_THE_UNLOAD: &str = "The second desk keeps the spare keys";

/// A harvest recorded before an unload is done after the load. The second
/// mob's agent has a note and retires; its exit interview is queued in the
/// second mob's store; the mob is unloaded before any dream. After its
/// load the mob's own dream harvests the note into the second mob's shared
/// memory and settles the interview.
#[tokio::test]
async fn a_harvest_recorded_before_an_unload_is_done_after_the_load() {
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let second = launched.hosted("second").await;
    let of_the_second = launched.store_of(&second);
    let realm = realm_of(&second);
    let noted = launched.remembers(&second, NOTED_BEFORE_THE_UNLOAD).await;
    let memory = launched
        .runtime_of(&second)
        .await
        .agent_memory_in_tests()
        .await
        .expect("the second mob keeps agent memory");
    let queued = memory
        .record_identity_retired_at(
            &super::mob_memory_tests::desk(),
            None,
            "retire",
            1_800_000_000_000,
        )
        .await;
    assert!(queued.is_ok(), "the exit interview is asked: {queued:?}");
    assert_eq!(
        exit_interviews_waiting_in(&of_the_second, &realm).await,
        1,
        "the exit interview waits in the second mob's store"
    );
    drop(memory);
    stored.scripted.answers_arrays_with(&serde_json::json!([{
        "record_id": noted,
        "verdict": "promote",
        "rationale": "the whole mob needs it",
    }]));
    launched.unloads(&[&second]).await;
    assert_eq!(
        exit_interviews_waiting_in(&of_the_second, &realm).await,
        1,
        "it still waits after the unload"
    );
    launched.ages_the_dreams_of(&second).await;
    let before = dreams_ended_in(&of_the_second, &realm).await;

    launched.loads(&second).await;
    assert_eq!(
        dreams_ended_in(&of_the_second, &realm).await,
        before + 1,
        "the mob's own dream ran after its load"
    );
    assert_eq!(
        exit_interviews_waiting_in(&of_the_second, &realm).await,
        0,
        "the exit interview is settled"
    );
    assert_eq!(
        titles_in(&of_the_second, &Engined::shared_scope(&realm, &second)).await,
        [NOTED_BEFORE_THE_UNLOAD],
        "and the note is shared memory of the second mob"
    );
    shut_down(&launched.host, &[&second]).await;
}

/// What was made below the launch's memory root for the mobs the launch
/// hosts: nothing, or the files that lie there.
fn made_for_hosted_mobs(stored: &StoredEngines) -> Option<Vec<String>> {
    let hosted = stored.memory_root().join(HOSTED_MOBS_DIR);
    if !hosted.exists() {
        return None;
    }
    let mut files = Vec::new();
    let mut dirs = vec![hosted.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.push(
                    path.strip_prefix(&hosted)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
            }
        }
    }
    files.sort();
    Some(files)
}

/// A mob that keeps no memory gets no memory file from its engines. A mob
/// beside the launch mob whose agents keep no memory is composed with
/// engines of its own, unloaded and loaded again, and an occurrence of the
/// host's dream passes it: at no step is a directory of hosted mobs made
/// below the launch's memory root. (A mob whose agents keep memory has its
/// file from its first agent's build, which reads it.)
#[tokio::test]
async fn a_mob_that_keeps_no_memory_gets_no_memory_file_from_its_engines() {
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let idle = launched.hosted_of(definition("idle")).await;
    assert!(
        launched
            .runtime_of(&idle)
            .await
            .agent_memory_in_tests()
            .await
            .is_some_and(|memory| memory.engines_in_tests() == (true, true)),
        "the mob is composed with a distiller and a steward of its own"
    );
    assert_eq!(
        made_for_hosted_mobs(&stored),
        None,
        "the composition of the mob, its agent's seat and what its first load asks made no \
         memory file"
    );
    launched.unloads(&[&idle]).await;
    assert_eq!(
        made_for_hosted_mobs(&stored),
        None,
        "the mob's unload made none"
    );
    launched.loads(&idle).await;
    assert_eq!(
        made_for_hosted_mobs(&stored),
        None,
        "the mob's load, which asks the dream a mob is owed, made none"
    );
    crate::memory_wiring::dream_for_the_host(
        &launched.steward,
        Some(&launched.host.hosted_mob_dreams()),
    )
    .await;
    assert_eq!(
        made_for_hosted_mobs(&stored),
        None,
        "an occurrence of the host's dream made none"
    );
    shut_down(&launched.host, &[&idle]).await;
}

/// On a host of one mob the dream occurrence dreams once, for the launch
/// mob, as before.
#[tokio::test]
async fn the_dream_occurrence_of_a_host_of_one_mob_dreams_once_as_before() {
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let launch_realm = AgentMemoryConfig::default().realm;
    an_exit_interview_waits_in(&launched.of_the_launch, &launch_realm).await;
    crate::memory_wiring::dream_for_the_host(
        &launched.steward,
        Some(&launched.host.hosted_mob_dreams()),
    )
    .await;
    assert_eq!(
        dreams_ended_in(&launched.of_the_launch, &launch_realm).await,
        1,
        "the launch mob's steward dreamed once"
    );
    assert!(
        !stored.memory_root().join(HOSTED_MOBS_DIR).exists(),
        "and no directory of hosted mobs was made"
    );
    shut_down(&launched.host, &[]).await;
}

/// The mob set destroys the mob `id`, and the runtime lets go of its entry.
pub(super) async fn destroyed_by_the_set(host: &UnifiedRuntime, id: &MobId) {
    let destroyed = Box::pin(set_of(host).mob_destroy(id)).await;
    assert!(
        destroyed.is_ok(),
        "the mob set destroys the mob: {:?}",
        destroyed.as_ref().err()
    );
    for _ in 0..200 {
        if !host.mob_entries_in_tests().contains(id) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !host.mob_entries_in_tests().contains(id),
        "the runtime lets go of the entry of the destroyed mob {id}"
    );
}

const NOTED_BEFORE_THE_DISTILLATION: &str = "The second desk has a bell";
const DISTILLED_AFTER_THE_UNLOAD: &str = "The second desk closes at five";

/// A distillation under way when its mob is unloaded writes nothing
/// afterwards. The second mob's desk has a note of its own, and a detached
/// distillation of the session its journal bound is at its model call when
/// the mob is unloaded. The run ended with the mob's identity plane: once
/// the mob is unloaded its distiller has no run under way and starts none,
/// nothing holds the mob's distiller or its steward any more (and so
/// nothing holds the mob's store or its handle through them), and when the
/// model then answers with a note to remember, nothing is written into the
/// realm of the mob that is not loaded.
#[tokio::test]
async fn a_distillation_under_way_when_its_mob_is_unloaded_writes_nothing_afterwards() {
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let second = launched.hosted("second").await;
    let of_the_second = launched.store_of(&second);
    let realm = realm_of(&second);
    launched
        .remembers(&second, NOTED_BEFORE_THE_DISTILLATION)
        .await;
    let handle = set_of(&launched.host)
        .handle_for(&second)
        .await
        .expect("the set serves the second mob");
    let bound = session_of_the_desk(&handle).await;
    drop(handle);
    let session = a_session_under(
        &stored.sessions,
        Some(bound),
        Some((&second, DESK)),
        "The desk was told that it closes at five.",
    )
    .await;
    stored.scripted.answers_arrays_with(&serde_json::json!([{
        "action": "remember",
        "kind": "fact",
        "title": DISTILLED_AFTER_THE_UNLOAD,
        "description": "When the second desk closes.",
        "body": "The second desk closes at five.",
        "tags": [],
        "epistemic": "observed",
        "evidence_range": [0, 0],
    }]));
    let asked_before = stored.scripted.asked().len();
    stored.scripted.holds();
    let memory = launched
        .runtime_of(&second)
        .await
        .agent_memory_in_tests()
        .await
        .expect("the second mob keeps agent memory");
    memory.spawn_rotation_distillation(
        &super::mob_memory_tests::desk(),
        &session,
        crate::memory::distiller::DistillCause::Retire,
    );
    let distiller = memory
        .distiller_in_tests()
        .expect("the second mob has a distiller of its own");
    drop(memory);
    assert!(
        stored.scripted.is_asked_after(asked_before).await,
        "the distillation of the desk's session is at its model call"
    );
    assert_eq!(
        distiller.detached_runs_in_tests(),
        (false, 1),
        "the distillation is a run of the mob's distiller, under way"
    );

    let steward = Arc::downgrade(
        &(launched.host.hosted_mob_dreams())()
            .into_iter()
            .find(|dream| dream.mob == second)
            .expect("the second mob has a steward of its own")
            .steward,
    );
    assert!(
        steward.upgrade().is_some() && Arc::strong_count(&distiller) > 1,
        "while the mob is loaded its identity plane holds its steward and its distiller"
    );

    launched.unloads(&[&second]).await;
    // The entry's end follows its leaving the table: the end of the mob's
    // identity plane is what ends the distiller's runs.
    let mut ended = distiller.detached_runs_in_tests();
    for _ in 0..200 {
        if ended == (true, 0) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        ended = distiller.detached_runs_in_tests();
    }
    assert_eq!(
        ended,
        (true, 0),
        "once the mob is unloaded its distiller has no run under way and starts none: the run \
         ended with the mob's identity plane"
    );
    let distiller = {
        let weak = Arc::downgrade(&distiller);
        drop(distiller);
        weak
    };
    // What held the mob's engines for its identity plane went with the
    // plane: nothing holds them, so nothing holds the mob's store or its
    // handle through them.
    for _ in 0..200 {
        if distiller.upgrade().is_none() && steward.upgrade().is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        distiller.upgrade().is_none(),
        "nothing holds the mob's distiller once the mob's identity plane ended"
    );
    assert!(
        steward.upgrade().is_none(),
        "and nothing holds the mob's steward"
    );
    stored.scripted.releases();

    assert_eq!(
        titles_in(
            &of_the_second,
            &MemoryScope::Identity {
                realm,
                identity: DESK.to_string(),
            }
        )
        .await,
        [NOTED_BEFORE_THE_DISTILLATION],
        "nothing was written into the realm of the mob after its unload"
    );
    shut_down(&launched.host, &[]).await;
}

const NOTED_BEFORE_THE_RELEASE: &str = "The second desk keeps the ledger";

/// A dream under way when its mob is released writes nothing into a mob
/// created under its id. The second mob's desk has a note and an exit
/// interview that waits; the host's dream occurrence is at the model call
/// of the second mob's dream when the mob set destroys the mob. The dream
/// ended with the mob's identity plane: the occurrence ends while the model
/// still holds its answer. The host then releases the mob's memory, and the
/// model answers that the note is to be shared: nothing makes the mob's
/// memory directory again, and a mob created under the id has no note, no
/// shared memory and no exit interview of the earlier mob. Its engines
/// read the session its own journal bound, and not the session of the
/// earlier mob's desk, which names the same mob id.
#[tokio::test]
async fn a_dream_under_way_when_its_mob_is_released_writes_nothing_into_a_mob_created_under_its_id()
{
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let of_the_second = definition_keeping_memory("second");
    let again = of_the_second.clone();
    let second = launched.hosted_of(of_the_second).await;
    let noted = launched.remembers(&second, NOTED_BEFORE_THE_RELEASE).await;
    // What the earlier mob's desk was told, under the session the earlier
    // mob's journal bound: a session that names this mob id.
    let handle = set_of(&launched.host)
        .handle_for(&second)
        .await
        .expect("the set serves the second mob");
    let of_the_earlier_desk = session_of_the_desk(&handle).await;
    drop(handle);
    let told_to_the_earlier_desk = a_session_under(
        &stored.sessions,
        Some(of_the_earlier_desk.clone()),
        Some((&second, DESK)),
        "said to the desk of the earlier mob",
    )
    .await;
    let memory = launched
        .runtime_of(&second)
        .await
        .agent_memory_in_tests()
        .await
        .expect("the second mob keeps agent memory");
    let queued = memory
        .record_identity_retired_at(
            &super::mob_memory_tests::desk(),
            None,
            "retire",
            1_800_000_000_000,
        )
        .await;
    assert!(queued.is_ok(), "the exit interview is asked: {queued:?}");
    drop(memory);
    stored.scripted.answers_arrays_with(&serde_json::json!([{
        "record_id": noted,
        "verdict": "promote",
        "rationale": "the whole mob needs it",
    }]));
    let asked_before = stored.scripted.asked().len();
    stored.scripted.holds();
    let steward = Arc::clone(&launched.steward);
    let hosted = launched.host.hosted_mob_dreams();
    let occurrence = tokio::spawn(async move {
        crate::memory_wiring::dream_for_the_host(&steward, Some(&hosted)).await;
    });
    assert!(
        stored.scripted.is_asked_after(asked_before).await,
        "the second mob's dream is at its model call"
    );

    destroyed_by_the_set(&launched.host, &second).await;
    // The model still holds its answer: the occurrence ends because the
    // mob's dream was stopped with the mob's identity plane, not because
    // the dream came to its end.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(20), occurrence).await;
    assert!(
        matches!(ended, Ok(Ok(()))),
        "the occurrence ends while the model holds its answer: the mob's dream ended with the mob"
    );
    let released = launched
        .host
        .mob_resolver()
        .release_memory_of_a_destroyed_mob(&second)
        .await;
    assert!(
        released.is_ok(),
        "the host releases the memory of the destroyed mob: {released:?}"
    );
    stored.scripted.releases();
    let directory = launched
        .of_the_launch
        .root_of_hosted_mob(&second)
        .expect("the directory of a mob with an id");
    assert!(
        !directory.exists(),
        "nothing made the released mob's memory directory again"
    );

    let reborn = launched.hosted_of(again).await;
    assert_eq!(reborn, second, "a mob under the freed id");
    let of_the_reborn = launched.store_of(&reborn);
    let realm = realm_of(&reborn);
    assert!(
        titles_in(
            &of_the_reborn,
            &MemoryScope::Identity {
                realm: realm.clone(),
                identity: DESK.to_string(),
            }
        )
        .await
        .is_empty(),
        "the desk of the mob created under the id has no note of the earlier mob's desk"
    );
    assert!(
        titles_in(&of_the_reborn, &Engined::shared_scope(&realm, &reborn))
            .await
            .is_empty(),
        "the mob created under the id has no shared memory of the earlier mob"
    );
    assert_eq!(
        exit_interviews_waiting_in(&of_the_reborn, &realm).await,
        0,
        "and no exit interview of it"
    );

    // The mob under the id reads the session its own journal bound, and
    // not the earlier desk's, which names the same mob id: the earlier
    // mob's journal went with it.
    let handle = set_of(&launched.host)
        .handle_for(&reborn)
        .await
        .expect("the set serves the mob created under the id");
    let of_the_new_desk = session_of_the_desk(&handle).await;
    drop(handle);
    assert_ne!(
        of_the_new_desk, of_the_earlier_desk,
        "the mob created under the id holds its desk bound to a session of its own"
    );
    let told_to_the_new_desk = a_session_under(
        &stored.sessions,
        Some(of_the_new_desk),
        Some((&reborn, DESK)),
        "said to the desk of the mob created under the id",
    )
    .await;
    let dream = (launched.host.hosted_mob_dreams())()
        .into_iter()
        .find(|dream| dream.mob == reborn)
        .expect("the mob created under the id has a steward of its own");
    let of_the_steward: Arc<dyn TranscriptSource> =
        Arc::clone(dream.steward.transcripts_in_tests());
    drop(dream);
    assert_eq!(
        held_by(&of_the_steward, &told_to_the_new_desk).await,
        ITS_OWN,
        "the session the journal of the mob created under the id bound"
    );
    assert_eq!(
        held_by(&of_the_steward, &told_to_the_earlier_desk).await,
        NOT_ITS_OWN,
        "the session of the earlier mob's desk, which names the same mob id"
    );
    drop(of_the_steward);
    shut_down(&launched.host, &[&reborn]).await;
}

/// A journal that cannot be read.
struct UnreadableJournal;

#[async_trait::async_trait]
impl crate::memory_wiring::BoundSessions for UnreadableJournal {
    async fn bound(
        &self,
        _session: &meerkat_core::types::SessionId,
    ) -> Result<Option<crate::memory_wiring::BoundMember>, String> {
        Err("the mob's journal did not read".to_string())
    }
}

/// A hosted mob's transcript source answers an error when the mob's journal
/// cannot be read: for a read and for whether the session is private to a
/// person. It is never "the store does not hold it" and never "private to
/// no one": nothing is distilled on a guess.
#[tokio::test]
async fn a_hosted_mobs_transcript_source_answers_an_error_when_its_journal_cannot_be_read() {
    let sessions = Arc::new(meerkat::MemoryStore::new());
    let session = a_session_under(&sessions, None, None, "said to someone").await;
    let source = crate::memory_wiring::hosted_mob_transcript_source_over(
        Arc::clone(&sessions) as Arc<dyn meerkat::SessionStore>,
        Arc::new(UnreadableJournal),
    );
    assert!(
        source.read(&session, 0).await.is_err(),
        "a read of a session the store holds answers an error"
    );
    assert!(
        source.private_to_a_person(&session).await.is_err(),
        "and whether it is private to a person is unknown"
    );
}

/// A dream's request for the transcript of a session is answered the same
/// words for a session the mob's journal never bound, for one no store
/// holds and for the session of a member private to a person: `not found`.
/// The second mob's dream asks evidence for the three; what its model is
/// shown next does not tell them apart, and says nothing of whose a session
/// is.
#[tokio::test]
async fn a_dreams_evidence_request_is_answered_alike_for_a_session_that_is_not_the_mobs_and_a_private_one()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let engined = launch_with_engines(&temp, config_of_the_launch()).await;
    let kept = &engined.kept;
    let never_bound = a_session_of(
        &engined.sessions,
        &kept.second_id,
        DESK,
        "said to the desk of an earlier mob of the id",
    )
    .await;
    let private = a_stored_session(
        &engined.sessions,
        Some(session_of_the_desk(&kept.second_handle().await).await),
        Some((&kept.second_id, DESK)),
        Some("console-caller:v1:owner"),
        "said to a member private to a person",
    )
    .await;
    let absent = meerkat_core::Session::new().id().to_string();
    engined.scripted.answers_objects_with(&serde_json::json!({
        "requests": [
            { "kind": "evidence", "session_id": never_bound },
            { "kind": "evidence", "session_id": private },
            { "kind": "evidence", "session_id": absent },
        ],
    }));
    engined.the_record_reads_the_mobs().await;
    let dream = engined
        .dream_of_the_second()
        .expect("the second mob has a steward of its own");
    dream.steward.note_session_completed();
    let outcome = dream.steward.dream_now().await;
    assert!(
        matches!(outcome, DreamOutcome::Completed(_)),
        "the second mob's dream runs: {outcome:?}"
    );

    let asked = engined.scripted.asked();
    for (session, whose) in [
        (&never_bound, "one the mob's journal never bound"),
        (&private, "the session of a member private to a person"),
        (&absent, "one no store holds"),
    ] {
        let answered = format!("(request unfulfillable: session '{session}' not found)");
        assert!(
            asked.iter().any(|prompt| prompt.contains(&answered)),
            "the request for a session that is {whose} is answered `not found`"
        );
        // Every line the model is shown about the session is that answer:
        // the id stands nowhere else in what it is sent.
        for prompt in &asked {
            assert_eq!(
                prompt.matches(session.as_str()).count(),
                prompt.matches(answered.as_str()).count(),
                "a session that is {whose} is named to the model in the `not found` answer alone"
            );
        }
    }
    assert!(
        asked
            .iter()
            .all(|prompt| !prompt.contains("is not one this mob's journal bound")),
        "and nothing the model is shown says whose a session is"
    );
    engined.kept.shut_down().await;
}

const NOTED_BEFORE_THE_MARK_WENT: &str = "The second desk has a spare lamp";

/// A hosted mob whose memory directory is not proved its own gets none of
/// its memory's engines. On a launch that runs both engines a mob remembers
/// a note, is owed a dream at its next load (an exit interview waits, its
/// dreams are old) and is unloaded; its directory then loses what says
/// whose it is. The request that loads the mob is refused the mob's
/// composition for that one reason, and the refusal stands before anything
/// of the mob's memory is made: the mob has no identity plane, the host's
/// dream occurrence holds no dream of it, no dream was asked at the load,
/// the engines' model was asked nothing, and the files below the hosted
/// mobs' directories are the ones that stood there.
#[tokio::test]
async fn a_mob_whose_memory_directory_is_not_proved_its_own_gets_no_engine_and_no_dream() {
    let stored = StoredEngines::new();
    let launched = stored.launch().await;
    let second = launched.hosted("second").await;
    launched
        .remembers(&second, NOTED_BEFORE_THE_MARK_WENT)
        .await;
    an_exit_interview_waits_in(&launched.store_of(&second), &realm_of(&second)).await;
    launched.ages_the_dreams_of(&second).await;
    launched.unloads(&[&second]).await;
    let dir = launched
        .of_the_launch
        .root_of_hosted_mob(&second)
        .expect("the mob's own directory");
    let removed = std::fs::remove_file(dir.join(crate::memory::sqlite_store::HOSTED_MOB_MARK));
    assert!(
        removed.is_ok(),
        "the directory loses what says whose it is: {removed:?}"
    );
    let stood = made_for_hosted_mobs(&stored);
    let asked_before = stored.scripted.asked().len();

    // The request that loads the mob, and with it the ask of its plane.
    let resolved = launched.host.resolve_mob(Some(&second)).await;
    assert!(
        resolved
            .as_ref()
            .is_ok_and(|mob| mob.identity_runtime_ref().is_none()),
        "the mob is served, with no identity plane"
    );
    let entry = launched
        .host
        .mob_entry_in_tests(&second)
        .expect("the loaded mob has its entry");
    let refusals = entry.memory_refusals_in_tests();
    assert!(
        !refusals.is_empty()
            && refusals.iter().all(|refusal| matches!(
                refusal,
                crate::memory::sqlite_store::HostedMemoryRefusal::NotMarked { .. }
            )),
        "every ask of the mob's composition is refused because nothing says whose the \
         directory is: {refusals:?}"
    );
    // Whatever a composition had started for the mob's memory would show
    // within these two seconds: the ask of an owed dream is a task of its
    // own, and a dream's first step asks the model.
    let mut dreams_of_the_mob = 0;
    let mut owed_asked = false;
    for _ in 0..20 {
        dreams_of_the_mob = (launched.host.hosted_mob_dreams())()
            .into_iter()
            .filter(|dream| dream.mob == second)
            .count();
        owed_asked = entry.owed_dream_is_asked_in_tests();
        if dreams_of_the_mob > 0 || owed_asked {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let asked_since = stored.scripted.asked().len() - asked_before;
    let stands = made_for_hosted_mobs(&stored);
    assert!(
        dreams_of_the_mob == 0 && !owed_asked && asked_since == 0 && stands == stood,
        "the refusal stands before the mob's engines, their transcript source and its dream \
         are made: the host's dream occurrence holds no dream of the mob \
         ({dreams_of_the_mob}), no dream was asked at its load ({owed_asked}), the engines' \
         model was asked nothing ({asked_since}), and the files below the hosted mobs' \
         directories are the ones that stood there ({stands:?} against {stood:?})"
    );
    shut_down(&launched.host, &[&second]).await;
}

const REMEMBERED_AFTER_THE_END: &str = "The third desk rings twice";

/// A mob of the host made before the launch handed its memory over: its
/// id, the entry the host's table holds for it once the follower's pass
/// made it, and the console events that entry named at its making. It has
/// no identity plane.
async fn a_mob_made_before_the_hand_over(
    launched: &LaunchWithMemory,
    tag: &str,
) -> (
    MobId,
    Arc<super::mob_entry::MobEntry>,
    super::ConsoleEventStore,
) {
    let host = &launched.host;
    let of_the_mob = definition_keeping_memory(tag);
    launched.roster.seat_desk_in(&of_the_mob.id);
    let id = second_mob(host, of_the_mob).await;
    let mut entry = host.mob_entry_in_tests(&id);
    for _ in 0..100 {
        if entry.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        entry = host.mob_entry_in_tests(&id);
    }
    let entry = entry.expect("the host's table holds an entry for a mob the set built for it");
    let its_own = entry
        .console_log
        .kept()
        .expect("an entry names its mob's console events at its making");
    assert!(
        !entry.identity_plane_is_composed(),
        "before the launch hands over, the mob '{id}' has no identity plane"
    );
    (id, entry, its_own)
}

/// A composition begun on an entry that already ended files nothing where
/// its mob's id names a store. Two mobs of the host get their entries
/// before the launch hands its memory over, so neither has an identity
/// plane; then every entry of the table ends, as at the end of the
/// runtime's shutdown, and the console events under both ids end with
/// them. The launch hands over, and a composition that had passed the
/// table's checks before that end begins on each ended entry.
///
/// Under the second mob's id no store stands: the composition makes none.
/// Under the third mob's id a store stands again, as after a mob made
/// under the id since: what the memory composed for the ended entry files
/// (a write that waits for a review) is not filed in it, and is filed in
/// the store the entry held from its making.
#[tokio::test]
async fn a_composition_begun_on_an_ended_entry_files_nothing_in_the_store_its_mobs_id_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (launched, to_hand_over) = launch_with_memory_not_handed_over(
        &temp,
        AgentMemoryConfig {
            llm_writes: AgentMemoryLlmWrites::Quarantined,
            ..config_of_the_launch()
        },
        &MemoryEnginesConfig::default(),
        |spec| spec,
    )
    .await;
    let host = &launched.host;
    let of_the_host = host.console_events();
    let (second, entry_of_the_second, _) =
        a_mob_made_before_the_hand_over(&launched, "second").await;
    let (third, entry_of_the_third, own_of_the_third) =
        a_mob_made_before_the_hand_over(&launched, "third").await;

    // Every entry of the table ends, and with each the console events under
    // its mob's id.
    host.other_mobs().close().await;
    assert!(
        of_the_host.mobs_with_a_store().is_empty(),
        "the console events of both mobs ended with their entries: {:?}",
        of_the_host.mobs_with_a_store()
    );
    to_hand_over.hand_over_to(host);
    // A store stands under the third mob's id again.
    let named_since = of_the_host.of_mob(Some(&third));
    assert!(
        !named_since.is_the_store(&own_of_the_third),
        "the store the third mob's id names now is not the one its ended entry held"
    );

    host.other_mobs()
        .compose_begun_before_the_end_in_tests(&entry_of_the_second)
        .await;
    host.other_mobs()
        .compose_begun_before_the_end_in_tests(&entry_of_the_third)
        .await;

    let with_a_store = of_the_host.mobs_with_a_store();
    let context_of_the_third = entry_of_the_third.identity_first_context.get().cloned();
    assert!(
        context_of_the_third.is_some(),
        "the composition begun on the third mob's ended entry attached its identity context"
    );
    let (mut in_its_own, mut in_the_named) = (Vec::new(), Vec::new());
    if let Some(context) = context_of_the_third {
        let handle = set_of(host)
            .handle_for(&third)
            .await
            .expect("the set serves the third mob");
        desk_calls_memory(
            &context,
            handle,
            serde_json::json!({
                "action": "remember",
                "title": REMEMBERED_AFTER_THE_END,
                "body": "The third desk rings twice before it opens.",
            }),
        )
        .await;
        in_its_own = memory_events_once(&own_of_the_third, 1).await;
        in_the_named = memory_events_in(&named_since).await;
    }
    assert!(
        with_a_store == [third.clone()] && in_the_named.is_empty() && in_its_own.len() == 1,
        "a composition begun on an ended entry made no store under the second mob's id (the \
         mobs with a store: {with_a_store:?}), filed nothing in the store the third mob's id \
         names ({in_the_named:?}), and filed its write in the store its entry held \
         ({in_its_own:?})"
    );
    shut_down(host, &[&second, &third]).await;
}
