//! Rows of the agent memory of a mob a host serves beside its launch mob.
//!
//! The fixture is the launch of the identity plane's rows, on a launch that
//! keeps agent memory in the bundled store: the launch mob's agents are
//! built with the launch's memory, as a gateway builds them, and the launch
//! hands its memory over for the mobs it hosts beside its launch mob. Each
//! row seats an agent of ONE name, `desk`, in the launch mob and in a second
//! mob, and reads what each is given and where each writes.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use meerkat_core::agent::AgentToolDispatcher;
use meerkat_mob::{MobDefinition, MobId};

use super::UnifiedRuntime;
use super::mob_entries_tests::{launch_composed, second_mob, set_of, shut_down};
use crate::gateway_wiring::{HostMobIdentities, HostMobMemory};
use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryProvider, AgentMemoryRecallRequest, AgentMemorySelection,
    MEMORY_TOOL_NAME, NewAgentMemory,
};
use crate::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentIdentity,
    AgentMemoryCustomizer, AgentMemoryRuntimeInjector, AgentRuntimeServices, DurabilityPolicy,
    DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider, RosterContext, RosterError, RosterProvider,
};
use crate::memory::capabilities::StewardStore;
use crate::memory::coordinator::{MobScopeResolver, StaticMobBinding};
use crate::memory::records::{MemoryAuthor, MemoryKind, MemoryScope, NewMemoryRecord};
use crate::memory::sqlite_store::{HOSTED_MOBS_DIR, SqliteAgentMemoryStore};
use crate::mob_handle_runtime::MobBootstrapSpec;

/// The one agent name of the rows: an agent of this name is seated in the
/// launch mob and in the second mob.
pub(super) const DESK: &str = "desk";

/// A definition with one profile whose agents keep memory, under an id of
/// its own.
pub(super) fn definition_keeping_memory(tag: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"mob-memory-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\nmemory = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with one profile that keeps memory")
}

pub(super) fn desk() -> AgentIdentity {
    AgentIdentity::parse(DESK).expect("identity")
}

/// The row of the agent `desk` a host configures: the fixture's `worker`
/// profile, nothing else.
pub(super) fn configured_desk() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: desk(),
        profile: meerkat_mob::ProfileName::from("worker"),
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

/// The roster provider of a launch: one for every mob of the host, which
/// answers the agent `desk` for each mob it was configured for.
#[derive(Default)]
pub(super) struct DeskRoster {
    mobs: Mutex<Vec<MobId>>,
}

impl DeskRoster {
    pub(super) fn seat_desk_in(&self, mob_id: &MobId) {
        self.mobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(mob_id.clone());
    }
}

#[async_trait::async_trait]
impl RosterProvider for DeskRoster {
    async fn roster(&self, context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        let seated = context.mob_definition.as_ref().is_some_and(|definition| {
            self.mobs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&definition.id)
        });
        Ok(if seated {
            vec![configured_desk()]
        } else {
            Vec::new()
        })
    }
}

/// A launch that keeps agent memory, before any second mob.
pub(super) struct LaunchWithMemory {
    pub(super) host: UnifiedRuntime,
    /// The launch's roster provider: one for every mob of the host.
    pub(super) roster: Arc<DeskRoster>,
    /// The launch's own store, over the launch's memory root.
    pub(super) of_the_launch: SqliteAgentMemoryStore,
    /// The configuration the launch mob's agent memory is built with.
    pub(super) config: AgentMemoryConfig,
    /// The launch mob's identity context.
    pub(super) launch_context: Arc<IdentityFirstRuntimeContext>,
    /// The tracker the launch store's write gate reads.
    pub(super) taint_of_the_launch: crate::memory::taint::SessionTaintTracker,
    /// The provider the launch mob's owners hold: the launch's store with
    /// its firewall.
    pub(super) provider_of_the_launch: Arc<dyn AgentMemoryProvider>,
}

/// A launch that keeps agent memory, with a second mob of the host.
pub(super) struct KeptMemory {
    pub(super) host: UnifiedRuntime,
    /// The launch's own store, over the launch's memory root.
    pub(super) of_the_launch: SqliteAgentMemoryStore,
    /// The configuration the launch mob's agent memory is built with.
    pub(super) config: AgentMemoryConfig,
    /// The launch mob's identity context.
    pub(super) launch_context: Arc<IdentityFirstRuntimeContext>,
    /// The tracker the launch store's write gate reads.
    pub(super) taint_of_the_launch: crate::memory::taint::SessionTaintTracker,
    /// The provider the launch mob's owners hold.
    pub(super) provider_of_the_launch: Arc<dyn AgentMemoryProvider>,
    pub(super) second_id: MobId,
}

/// The agent memory configuration of the rows' launch: every record of an
/// agent is delivered to it.
pub(super) fn config_of_the_launch() -> AgentMemoryConfig {
    AgentMemoryConfig {
        selection: AgentMemorySelection::Always,
        ..AgentMemoryConfig::default()
    }
}

/// The launch's memory, as a gateway builds its launch mob's: the bundled
/// store with its firewall and the runtime's own memory event sink, the
/// engines `engines` turns on (over the state directory `state` and a
/// session store of their own), the customizer that wraps none with the
/// launch mob's memory, and the injector of the launch mob's identity
/// runtime. With them, the stack's tracker and its provider.
fn memory_of_the_launch_mob(
    host: &UnifiedRuntime,
    store: &SqliteAgentMemoryStore,
    config: &AgentMemoryConfig,
    launch_mob: &MobId,
    engines: &crate::memory_wiring::MemoryEnginesConfig,
    state: &std::path::Path,
) -> (
    Arc<dyn AgentCustomizer>,
    AgentMemoryRuntimeInjector,
    crate::memory::taint::SessionTaintTracker,
    Arc<dyn AgentMemoryProvider>,
) {
    let stack = crate::memory_wiring::attach_memory_engines(
        Arc::new(store.clone()),
        config,
        engines,
        crate::memory_wiring::MemoryStackSeams {
            persistent_state: Some(state.to_path_buf()),
            transcript_store: Some(Arc::new(meerkat_store::MemoryStore::new())),
            event_sink: Some(host.memory_event_sink()),
            ..Default::default()
        },
    )
    .expect("the launch's memory stack");
    let resolver: Arc<dyn MobScopeResolver> = Arc::new(StaticMobBinding {
        realm: config.realm.clone(),
        mob: launch_mob.to_string(),
    });
    let customizer = AgentMemoryCustomizer::wrap(None, stack.provider.clone(), config.clone())
        .with_mob_resolver(Some(Arc::clone(&resolver)));
    let mut injector = AgentMemoryRuntimeInjector::new(stack.provider.clone(), config.clone())
        .with_taint_tracker(stack.taint.clone())
        .with_mob_resolver(Some(resolver));
    if let Some(distiller) = stack.distiller {
        injector = injector.with_distiller(distiller);
    }
    if let Some(steward) = stack.steward {
        injector = injector.with_steward(steward);
    }
    (Arc::new(customizer), injector, stack.taint, stack.provider)
}

/// A launch that keeps agent memory in the bundled store under `temp`, its
/// launch mob's agent `desk` built with that memory, which hands its memory
/// over for the mobs it hosts beside its launch mob; and a second mob of
/// the host whose roster holds an agent `desk` too.
pub(super) async fn launch_keeping_memory(temp: &tempfile::TempDir) -> KeptMemory {
    launch_keeping_memory_as(temp, config_of_the_launch()).await
}

/// [`launch_keeping_memory`] with the agent memory configuration `config`.
pub(super) async fn launch_keeping_memory_as(
    temp: &tempfile::TempDir,
    config: AgentMemoryConfig,
) -> KeptMemory {
    with_a_second_mob(launch_with_memory(temp, config, |spec| spec).await).await
}

/// `launched`, with a second mob of the host whose roster holds an agent
/// `desk` too.
pub(super) async fn with_a_second_mob(launched: LaunchWithMemory) -> KeptMemory {
    let LaunchWithMemory {
        host,
        roster,
        of_the_launch,
        config,
        launch_context,
        taint_of_the_launch,
        provider_of_the_launch,
    } = launched;
    let of_the_second = definition_keeping_memory("second");
    roster.seat_desk_in(&of_the_second.id);
    let second_id = second_mob(&host, of_the_second).await;
    // The values a row makes for the second mob's store are made for the
    // mob as the set holds it, whoever touches its directory first.
    of_the_launch.hosted_mob_stands_at_in_tests(
        &second_id,
        set_of(&host)
            .handle_for(&second_id)
            .await
            .expect("the set serves the second mob")
            .incarnation(),
    );
    KeptMemory {
        host,
        of_the_launch,
        config,
        launch_context,
        taint_of_the_launch,
        provider_of_the_launch,
        second_id,
    }
}

/// [`launch_keeping_memory`] before any second mob, with what `compose`
/// makes of the launch's spec before the bootstrap (a row that needs what a
/// gateway puts on its mobs says it there).
pub(super) async fn launch_with_memory(
    temp: &tempfile::TempDir,
    config: AgentMemoryConfig,
    compose: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> LaunchWithMemory {
    launch_with_memory_and_engines(
        temp,
        config,
        &crate::memory_wiring::MemoryEnginesConfig::default(),
        compose,
    )
    .await
}

/// [`launch_with_memory`] on a launch whose memory runs the engines
/// `engines` turns on, for its launch mob.
pub(super) async fn launch_with_memory_and_engines(
    temp: &tempfile::TempDir,
    config: AgentMemoryConfig,
    engines: &crate::memory_wiring::MemoryEnginesConfig,
    compose: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> LaunchWithMemory {
    let (launched, to_hand_over) =
        launch_with_memory_not_handed_over(temp, config, engines, compose).await;
    to_hand_over.hand_over_to(&launched.host);
    launched
}

/// [`launch_with_memory_and_engines`] up to its hand-over: the launch, and
/// what it hands to its runtime for the mobs it hosts beside its launch
/// mob, not handed over yet. A mob made before the hand-over gets its
/// entry and no identity plane.
pub(super) async fn launch_with_memory_not_handed_over(
    temp: &tempfile::TempDir,
    config: AgentMemoryConfig,
    engines: &crate::memory_wiring::MemoryEnginesConfig,
    compose: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> (LaunchWithMemory, HostMobIdentities) {
    let (mut host, _sessions, _machine, ()) = launch_composed(
        temp,
        definition_keeping_memory("host"),
        meerkat::Config::default(),
        |_| (),
        compose,
    )
    .await;
    let launch_mob = host.mob_handle().mob_id().clone();
    let of_the_launch = SqliteAgentMemoryStore::open(temp.path().join("agent-memory"))
        .expect("the launch's memory store");
    let (customizer, injector, taint_of_the_launch, provider_of_the_launch) =
        memory_of_the_launch_mob(
            &host,
            &of_the_launch,
            &config,
            &launch_mob,
            engines,
            temp.path(),
        );

    let handle = host.mob_runtime().handle().observer();
    let identity_runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "mob-memory-launch".to_string(),
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

    // The launch hands over what it gave its launch mob: its customizer as
    // it was before the memory wrap (none here) and its memory.
    let to_hand_over = HostMobIdentities::new(Arc::clone(&roster) as Arc<dyn RosterProvider>, true)
        .with_agent_memory_per_mob(Some(HostMobMemory::new(
            of_the_launch.clone(),
            config.clone(),
            launch_mob,
        )));
    (
        LaunchWithMemory {
            host,
            roster,
            of_the_launch,
            config,
            launch_context,
            taint_of_the_launch,
            provider_of_the_launch,
        },
        to_hand_over,
    )
}

impl KeptMemory {
    /// The memory realm of the second mob.
    pub(super) fn realm_of_the_second(&self) -> String {
        format!("mob.{}", self.second_id)
    }

    /// A store value over the second mob's own directory, for a row to seed
    /// and to read: what the mob's own owners write is read through it.
    pub(super) fn of_the_second(&self) -> SqliteAgentMemoryStore {
        self.of_the_launch
            .of_hosted_mob_in_tests(&self.second_id)
            .expect("the second mob's store")
    }

    /// The second mob's identity context, once the mob resolves.
    pub(super) async fn second_context(
        &self,
    ) -> Option<crate::identity_first::PublicIdentityFirstContext> {
        self.host
            .resolve_mob(Some(&self.second_id))
            .await
            .ok()
            .and_then(|mob| mob.identity_first_context())
    }

    /// The handle of the second mob.
    pub(super) async fn second_handle(&self) -> meerkat_mob::MobHandle {
        set_of(&self.host)
            .handle_for(&self.second_id)
            .await
            .expect("the set serves the second mob")
    }

    pub(super) async fn shut_down(self) {
        shut_down(&self.host, &[&self.second_id]).await;
    }
}

pub(super) fn empty_draft() -> AgentBuildDraft {
    AgentBuildDraft {
        compaction_curator: Default::default(),
        model: None,
        system_prompt: None,
        additional_instructions: Vec::new(),
        labels: Default::default(),
        app_context: None,
        external_tools: Vec::new(),
        local_external_tools: Default::default(),
        provider_params: None,
    }
}

/// What the customizer of `context` makes of a build of the agent `desk`
/// of the mob `handle` serves: the draft the mob's identity runtime would
/// build the agent's session from.
pub(super) async fn built_desk(
    context: &IdentityFirstRuntimeContext,
    handle: meerkat_mob::MobHandle,
) -> AgentBuildDraft {
    let customizer = context
        .customizer
        .clone()
        .expect("the mob's identity context has a customizer");
    let mut draft = empty_draft();
    customizer
        .customize_build(
            &AgentBuildContext {
                identity: desk(),
                active_peers: Vec::new(),
                managed_edges: Vec::new(),
                runtime_services: AgentRuntimeServices::new(handle),
            },
            &configured_desk(),
            &mut draft,
        )
        .await
        .expect("the customizer builds the agent");
    draft
}

/// Everything a build was given as instructions, as one text.
pub(super) fn instructions_of(draft: &AgentBuildDraft) -> String {
    draft.additional_instructions.join("\n")
}

/// Call the `memory` tool a build offers with `args`: its answer's text,
/// and whether it is an error.
pub(super) async fn call_memory_tool(
    draft: &AgentBuildDraft,
    args: serde_json::Value,
) -> (String, bool) {
    let dispatcher: Arc<dyn AgentToolDispatcher> = draft
        .local_external_tools
        .dispatcher()
        .expect("the build offers tools");
    let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("tool arguments");
    let outcome = dispatcher
        .dispatch(meerkat_core::types::ToolCallView {
            id: "call-1",
            name: MEMORY_TOOL_NAME,
            args: &raw,
        })
        .await
        .expect("the memory tool answers");
    let text = outcome
        .result
        .content
        .iter()
        .filter_map(|block| match block {
            meerkat_core::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, outcome.result.is_error)
}

pub(super) fn a_note(title: &str) -> NewAgentMemory {
    NewAgentMemory {
        title: title.to_string(),
        body: format!("{title}, in full."),
        tags: Vec::new(),
    }
}

/// The titles `store` recalls for the agent `desk` in `realm`, sorted.
pub(super) async fn recalled(store: &SqliteAgentMemoryStore, realm: &str) -> Vec<String> {
    let mut titles = store
        .recall(AgentMemoryRecallRequest {
            identity: desk(),
            realm: realm.to_string(),
            query_text: None,
            query_terms: Vec::new(),
            selection: AgentMemorySelection::Always,
            max_entries: 64,
        })
        .await
        .expect("the store recalls")
        .into_iter()
        .map(|record| record.title)
        .collect::<Vec<_>>();
    titles.sort();
    titles
}

const OF_THE_LAUNCH_DESK: &str = "The launch desk keeps the blue ledger";
const OF_THE_SECOND_DESK: &str = "The second desk keeps the green ledger";

/// Seed one note of each mob's agent `desk`, each in its own mob's store:
/// the launch mob's through the provider the launch mob's owners hold.
async fn seed_a_note_of_each_desk(kept: &KeptMemory) {
    kept.provider_of_the_launch
        .remember(&kept.config.realm, &desk(), a_note(OF_THE_LAUNCH_DESK))
        .await
        .expect("the launch mob's note");
    kept.of_the_second()
        .remember(
            &kept.realm_of_the_second(),
            &desk(),
            a_note(OF_THE_SECOND_DESK),
        )
        .await
        .expect("the second mob's note");
}

/// Two mobs of one host each have an agent `desk`, and each has notes of
/// its own. Each agent is built with its own notes and with none of the
/// other's: the launch mob's from the launch's store, the second mob's
/// from the second mob's own.
#[tokio::test]
async fn two_mobs_agents_of_one_name_are_each_built_with_their_own_identity_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    seed_a_note_of_each_desk(&kept).await;

    let of_the_launch = instructions_of(
        &built_desk(
            &kept.launch_context,
            kept.host.mob_runtime().handle().observer(),
        )
        .await,
    );
    assert!(
        of_the_launch.contains(OF_THE_LAUNCH_DESK) && !of_the_launch.contains(OF_THE_SECOND_DESK),
        "the launch mob's agent is built with its own note alone: {of_the_launch}"
    );

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let of_the_second =
            instructions_of(&built_desk(&second_context, kept.second_handle().await).await);
        assert!(
            of_the_second.contains(OF_THE_SECOND_DESK)
                && !of_the_second.contains(OF_THE_LAUNCH_DESK),
            "the second mob's agent is built with its own note alone: {of_the_second}"
        );
    }
    kept.shut_down().await;
}

/// What an injector delivers to the agent `desk` at a turn, as one text.
async fn given_at_a_turn(injector: &AgentMemoryRuntimeInjector, session: &str) -> String {
    injector
        .inject_for_turn(
            &desk(),
            Some(session),
            &meerkat_core::ContentInput::Text("which ledger do you keep?".to_string()),
        )
        .await
        .expect("the turn's delivery")
        .iter()
        .map(meerkat_core::ContentInput::text_content)
        .collect::<Vec<_>>()
        .join("\n")
}

/// So at a turn: each mob's identity runtime delivers its own agent's notes
/// to its agent `desk`, and none of the other mob's agent of that name.
#[tokio::test]
async fn two_mobs_agents_of_one_name_are_each_given_their_own_identity_memory_at_a_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    seed_a_note_of_each_desk(&kept).await;

    let of_the_launch = kept.launch_context.runtime.agent_memory_in_tests().await;
    assert!(
        of_the_launch.is_some(),
        "the launch mob's identity runtime delivers agent memory"
    );
    if let Some(injector) = of_the_launch {
        let given = given_at_a_turn(&injector, "session-of-the-launch-desk").await;
        assert!(
            given.contains(OF_THE_LAUNCH_DESK) && !given.contains(OF_THE_SECOND_DESK),
            "the launch mob's agent is given its own note alone at a turn: {given}"
        );
    }

    let second_runtime = kept
        .second_context()
        .await
        .map(|context| Arc::clone(&context.runtime));
    assert!(
        second_runtime.is_some(),
        "the second mob of a launch that keeps agent memory has an identity runtime"
    );
    if let Some(second_runtime) = second_runtime {
        assert_eq!(
            second_runtime.agent_memory_realm().await,
            Some(kept.realm_of_the_second()),
            "the second mob's identity runtime keeps agent memory under the mob's own realm"
        );
        let of_the_second = second_runtime.agent_memory_in_tests().await;
        assert!(
            of_the_second.is_some(),
            "the second mob's identity runtime delivers agent memory"
        );
        if let Some(injector) = of_the_second {
            let given = given_at_a_turn(&injector, "session-of-the-second-desk").await;
            assert!(
                given.contains(OF_THE_SECOND_DESK) && !given.contains(OF_THE_LAUNCH_DESK),
                "the second mob's agent is given its own note alone at a turn: {given}"
            );
        }
    }
    kept.shut_down().await;
}

/// An agent of the second mob recalls and records in its own mob's store.
/// Its `memory` tool recalls its own note and not the launch mob's agent's;
/// what it remembers is kept under its own name in its mob's realm, in its
/// mob's own directory; the launch's store holds what it held; and a host
/// that asks the second mob's identity runtime for the agent's memory is
/// answered from that mob's store.
#[tokio::test]
async fn an_agent_of_a_hosted_mob_recalls_and_records_in_its_own_mobs_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    seed_a_note_of_each_desk(&kept).await;
    let realm_of_the_second = kept.realm_of_the_second();

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let draft = built_desk(&second_context, kept.second_handle().await).await;
        let (recalled_by_the_tool, failed) =
            call_memory_tool(&draft, serde_json::json!({"action": "recall"})).await;
        assert!(
            !failed
                && recalled_by_the_tool.contains(OF_THE_SECOND_DESK)
                && !recalled_by_the_tool.contains(OF_THE_LAUNCH_DESK),
            "the second mob's agent recalls its own note alone: {recalled_by_the_tool}"
        );

        let (answer, failed) = call_memory_tool(
            &draft,
            serde_json::json!({
                "action": "remember",
                "title": "The second desk closes at six",
                "body": "The second desk closes at six on weekdays.",
            }),
        )
        .await;
        assert!(!failed, "the second mob's agent remembers: {answer}");
        assert_eq!(
            recalled(&kept.of_the_second(), &realm_of_the_second).await,
            [
                "The second desk closes at six".to_string(),
                OF_THE_SECOND_DESK.to_string()
            ],
            "what it remembered is kept under its name in its mob's realm, in its mob's store"
        );

        let asked_by_the_host = second_context
            .runtime
            .recall_agent_memory(AgentMemoryRecallRequest {
                identity: desk(),
                realm: realm_of_the_second.clone(),
                query_text: None,
                query_terms: Vec::new(),
                selection: AgentMemorySelection::Always,
                max_entries: 64,
            })
            .await
            .map(|records| {
                let mut titles = records
                    .into_iter()
                    .map(|record| record.title)
                    .collect::<Vec<_>>();
                titles.sort();
                titles
            });
        assert_eq!(
            asked_by_the_host.as_ref().map_err(ToString::to_string),
            Ok(&vec![
                "The second desk closes at six".to_string(),
                OF_THE_SECOND_DESK.to_string()
            ]),
            "a host that asks the second mob's identity runtime is answered from that mob's store"
        );
    }

    assert_eq!(
        recalled(&kept.of_the_launch, &kept.config.realm).await,
        [OF_THE_LAUNCH_DESK.to_string()],
        "the launch mob's agent's memory holds what it held"
    );
    assert!(
        recalled(&kept.of_the_launch, &realm_of_the_second)
            .await
            .is_empty(),
        "the launch's store holds nothing under the second mob's realm"
    );
    assert!(
        kept.taint_of_the_launch.identity_taint(DESK).is_none(),
        "what the second mob's agent did marks nobody in the launch's memory"
    );
    assert_eq!(
        kept.of_the_second().root(),
        temp.path()
            .join("agent-memory")
            .join(HOSTED_MOBS_DIR)
            .join(kept.second_id.as_str()),
        "the second mob's store is its own directory below the launch's memory root"
    );
    kept.shut_down().await;
}

/// A record shared by a mob's agents, written by the application.
fn a_shared_record(title: &str) -> NewMemoryRecord {
    NewMemoryRecord {
        kind: MemoryKind::Fact,
        title: title.to_string(),
        description: format!("{title}, in short."),
        body: format!("{title}, in full."),
        tags: Vec::new(),
        evidence: Vec::new(),
        verification: None,
    }
}

const SHARED_BY_THE_LAUNCH_MOB: &str = "The launch mob files claims on Mondays";
const SHARED_BY_THE_SECOND_MOB: &str = "The second mob files claims on Fridays";

/// Each mob has shared memory of its own. An agent of the second mob is
/// listed its own mob's shared record and not the launch mob's, and the
/// launch mob's agent of the same name the launch mob's and not the second
/// mob's.
#[tokio::test]
async fn an_agent_of_a_hosted_mob_is_listed_its_own_mobs_shared_memory_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    kept.of_the_launch
        .remember_authored(
            &MemoryScope::Mob {
                realm: kept.config.realm.clone(),
                mob: launch_mob.to_string(),
            },
            a_shared_record(SHARED_BY_THE_LAUNCH_MOB),
            MemoryAuthor::Application,
        )
        .await
        .expect("the launch mob's shared record");
    kept.of_the_second()
        .remember_authored(
            &MemoryScope::Mob {
                realm: kept.realm_of_the_second(),
                mob: kept.second_id.to_string(),
            },
            a_shared_record(SHARED_BY_THE_SECOND_MOB),
            MemoryAuthor::Application,
        )
        .await
        .expect("the second mob's shared record");

    let of_the_launch = instructions_of(
        &built_desk(
            &kept.launch_context,
            kept.host.mob_runtime().handle().observer(),
        )
        .await,
    );
    assert!(
        of_the_launch.contains(SHARED_BY_THE_LAUNCH_MOB)
            && !of_the_launch.contains(SHARED_BY_THE_SECOND_MOB),
        "the launch mob's agent is listed its own mob's shared record alone: {of_the_launch}"
    );

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let of_the_second =
            instructions_of(&built_desk(&second_context, kept.second_handle().await).await);
        assert!(
            of_the_second.contains(SHARED_BY_THE_SECOND_MOB)
                && !of_the_second.contains(SHARED_BY_THE_LAUNCH_MOB),
            "the second mob's agent is listed its own mob's shared record alone: {of_the_second}"
        );
    }
    kept.shut_down().await;
}

/// A proposal of the second mob's agent to its mob is filed for that mob,
/// in that mob's store: the launch's store holds no proposal of it.
#[tokio::test]
async fn a_proposal_of_a_hosted_mobs_agent_is_filed_for_its_own_mob_in_its_own_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory(&temp).await;
    let realm_of_the_second = kept.realm_of_the_second();

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let draft = built_desk(&second_context, kept.second_handle().await).await;
        let (answer, failed) = call_memory_tool(
            &draft,
            serde_json::json!({
                "action": "propose_to_mob",
                "title": "Claims are filed on Fridays",
                "body": "The second mob files its claims on Fridays.",
            }),
        )
        .await;
        assert!(
            !failed,
            "the second mob's agent proposes to its mob: {answer}"
        );

        let filed = kept
            .of_the_second()
            .pending_proposals(&realm_of_the_second, 16)
            .await
            .expect("the second mob's proposals");
        assert_eq!(
            filed
                .iter()
                .map(|proposal| (proposal.scope.clone(), proposal.record.title.clone()))
                .collect::<Vec<_>>(),
            [(
                MemoryScope::Mob {
                    realm: realm_of_the_second.clone(),
                    mob: kept.second_id.to_string(),
                },
                "Claims are filed on Fridays".to_string()
            )],
            "the proposal is filed for the second mob, in the second mob's store"
        );
    }
    for realm in [&kept.config.realm, &realm_of_the_second] {
        assert!(
            kept.of_the_launch
                .pending_proposals(realm, 16)
                .await
                .expect("the launch's proposals")
                .is_empty(),
            "the launch's store holds no proposal under the realm {realm:?}"
        );
    }
    kept.shut_down().await;
}

/// A host of one mob makes no directory for hosted mobs: its launch mob's
/// agent is built with its memory from the launch's root as before, and
/// the launch's memory root holds its realm file and nothing of another
/// mob.
#[tokio::test]
async fn a_host_of_one_mob_makes_no_hosted_memory_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let LaunchWithMemory {
        host,
        of_the_launch,
        config,
        launch_context,
        ..
    } = launch_with_memory(&temp, config_of_the_launch(), |spec| spec).await;
    of_the_launch
        .remember(&config.realm, &desk(), a_note(OF_THE_LAUNCH_DESK))
        .await
        .expect("the launch mob's note");

    let built =
        instructions_of(&built_desk(&launch_context, host.mob_runtime().handle().observer()).await);
    assert!(
        built.contains(OF_THE_LAUNCH_DESK),
        "the launch mob's agent is built with its note: {built}"
    );
    assert!(
        !temp
            .path()
            .join("agent-memory")
            .join(HOSTED_MOBS_DIR)
            .exists(),
        "a host of one mob has no directory of hosted mobs' memory"
    );
    assert_eq!(
        of_the_launch.known_realms().expect("the launch's realms"),
        std::slice::from_ref(&config.realm),
        "the launch's memory root holds its launch mob's realm alone"
    );
    shut_down(&host, &[]).await;
}

/// The memory events `events` holds, by event type.
pub(super) async fn memory_events_in(events: &super::ConsoleEventStore) -> Vec<String> {
    events
        .replay_all(None)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|event| event.event_type)
        .filter(|event_type| event_type.starts_with("memory."))
        .collect()
}

/// [`memory_events_in`], once `events` holds `count` memory events or
/// after five seconds: a memory event is filed by a task of its own.
pub(super) async fn memory_events_once(
    events: &super::ConsoleEventStore,
    count: usize,
) -> Vec<String> {
    for _ in 0..50 {
        let filed = memory_events_in(events).await;
        if filed.len() >= count {
            return filed;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    memory_events_in(events).await
}

/// The memory events of a hosted mob are filed under that mob. On a launch
/// whose agents' writes wait for a review, what the second mob's agent
/// remembers is quarantined in its own mob's store and the event of it is
/// filed in the second mob's console events, not in the launch mob's; what
/// the launch mob's agent remembers is filed in the launch mob's, as before.
#[tokio::test]
async fn the_memory_events_of_a_hosted_mob_are_filed_under_that_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let kept = launch_keeping_memory_as(
        &temp,
        AgentMemoryConfig {
            llm_writes: crate::identity_first::agent_memory::AgentMemoryLlmWrites::Quarantined,
            ..config_of_the_launch()
        },
    )
    .await;
    let of_the_host = kept.host.console_events();
    let of_the_second = of_the_host.of_mob(Some(&kept.second_id));

    let launch_draft = built_desk(
        &kept.launch_context,
        kept.host.mob_runtime().handle().observer(),
    )
    .await;
    let (answer, failed) = call_memory_tool(
        &launch_draft,
        serde_json::json!({
            "action": "remember",
            "title": "The launch desk opens at eight",
            "body": "The launch desk opens at eight on weekdays.",
        }),
    )
    .await;
    assert!(!failed, "the launch mob's agent remembers: {answer}");
    assert_eq!(
        memory_events_once(&of_the_host, 1).await,
        ["memory.write.quarantined"],
        "the launch mob's write is filed in the launch mob's console events"
    );

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let draft = built_desk(&second_context, kept.second_handle().await).await;
        let (answer, failed) = call_memory_tool(
            &draft,
            serde_json::json!({
                "action": "remember",
                "title": "The second desk closes at six",
                "body": "The second desk closes at six on weekdays.",
            }),
        )
        .await;
        assert!(!failed, "the second mob's agent remembers: {answer}");
        assert_eq!(
            memory_events_once(&of_the_second, 1).await,
            ["memory.write.quarantined"],
            "the second mob's write is filed in the second mob's console events"
        );
        assert!(
            recalled(&kept.of_the_second(), &kept.realm_of_the_second())
                .await
                .is_empty(),
            "and it waits for a review in the second mob's store: no agent recalls it"
        );
    }
    assert_eq!(
        memory_events_in(&of_the_host).await,
        ["memory.write.quarantined"],
        "the launch mob's console events hold the launch mob's event alone"
    );
    kept.shut_down().await;
}

/// What a row tells an agent to remember in a turn.
fn remember_ask(fact: &str) -> String {
    format!("Remember this: {fact}.")
}

/// The id of the tool call that recalls before a fact is remembered.
fn recall_call_id(fact: &str) -> String {
    format!("call-recall-{}", fact.replace(' ', "-"))
}

/// The id of the tool call that remembers a fact.
fn remember_call_id(fact: &str) -> String {
    format!("call-remember-{}", fact.replace(' ', "-"))
}

const TOLD_TO_THE_LAUNCH_DESK: &str = "the launch desk sorts mail at nine";
const TOLD_TO_THE_SECOND_DESK: &str = "the second desk sorts mail at four";

/// A members' model that calls its member's own `memory` tool twice for
/// the newest fact its conversation tells it to remember
/// ([`remember_ask`]): first to recall what its member holds, then to
/// remember the fact. It answers every other request in text, and keeps
/// every request it was sent, rendered.
#[derive(Clone, Default)]
struct RemembersWhatItIsTold {
    asked: Arc<Mutex<Vec<String>>>,
}

impl RemembersWhatItIsTold {
    /// The requests the model was sent whose messages hold `text`.
    fn asked_with(&self, text: &str) -> Vec<String> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|request| request.contains(text))
            .cloned()
            .collect()
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for RemembersWhatItIsTold {
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
        // The newest fact the conversation tells the member to remember:
        // a recall first, then the fact remembered, each once. A request
        // after a tool call carries the call, by its id, after the ask.
        let rendered = serde_json::to_string(&request.messages).unwrap_or_default();
        let call = [TOLD_TO_THE_LAUNCH_DESK, TOLD_TO_THE_SECOND_DESK]
            .into_iter()
            .filter_map(|fact| rendered.rfind(&remember_ask(fact)).map(|at| (at, fact)))
            .max_by_key(|(at, _)| *at)
            .and_then(|(at, fact)| {
                let after = rendered.get(at..).unwrap_or_default();
                if !after.contains(&recall_call_id(fact)) {
                    Some((
                        recall_call_id(fact),
                        serde_json::json!({"action": "recall"}),
                    ))
                } else if !after.contains(&remember_call_id(fact)) {
                    Some((
                        remember_call_id(fact),
                        serde_json::json!({
                            "action": "remember",
                            "title": fact,
                            "body": format!("As told in a turn: {fact}."),
                        }),
                    ))
                } else {
                    None
                }
            });
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(rendered);
        let (lead, stop_reason) = match call {
            Some((id, args)) => (
                meerkat_client::LlmEvent::ToolCallComplete {
                    id,
                    name: MEMORY_TOOL_NAME.to_string(),
                    args,
                    meta: None,
                },
                meerkat_core::StopReason::ToolUse,
            ),
            None => (
                meerkat_client::LlmEvent::TextDelta {
                    delta: "noted".to_string(),
                    meta: None,
                },
                meerkat_core::StopReason::EndTurn,
            ),
        };
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            stop_reason,
        );
        Box::pin(futures::stream::iter(vec![Ok(lead), Ok(usage), Ok(done)]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// [`recalled`], once `store` recalls a record titled `title` in `realm`
/// or after thirty seconds: a turn's tool call lands after its send.
async fn recalled_once(store: &SqliteAgentMemoryStore, realm: &str, title: &str) -> Vec<String> {
    for _ in 0..300 {
        let titles = recalled(store, realm).await;
        if titles.iter().any(|recalled| recalled == title) {
            return titles;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    recalled(store, realm).await
}

/// A turn of each mob's agent `desk`, sent through that mob's own identity
/// runtime, with a model that calls the `memory` tool its member's build
/// bound: a recall, then what it was told. What each agent's model is sent
/// back for its recall holds that agent's own note and none of the other
/// mob's agent's, and what each remembers in its turn is kept in its own
/// mob's store and in no other: the launch mob's agent's in the launch's
/// root, the second mob's agent's in the second mob's directory.
#[tokio::test]
async fn a_turn_of_each_mobs_agent_of_one_name_recalls_and_records_its_own_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = RemembersWhatItIsTold::default();
    let of_the_members = model.clone();
    let kept = with_a_second_mob(
        launch_with_memory(&temp, config_of_the_launch(), move |spec| {
            spec.with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(of_the_members)),
            })
        })
        .await,
    )
    .await;
    seed_a_note_of_each_desk(&kept).await;
    let realm_of_the_second = kept.realm_of_the_second();
    let text = |content: String| meerkat_core::ContentInput::Text(content);

    let to_the_launch_desk = kept
        .launch_context
        .runtime
        .send(&desk(), &text(remember_ask(TOLD_TO_THE_LAUNCH_DESK)))
        .await;
    assert!(
        to_the_launch_desk.is_ok(),
        "the send to the launch mob's agent: {:?}",
        to_the_launch_desk.as_ref().err()
    );
    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let to_the_second_desk = second_context
            .runtime
            .send(&desk(), &text(remember_ask(TOLD_TO_THE_SECOND_DESK)))
            .await;
        assert!(
            to_the_second_desk.is_ok(),
            "the send to the second mob's agent: {:?}",
            to_the_second_desk.as_ref().err()
        );
        assert_eq!(
            recalled_once(
                &kept.of_the_second(),
                &realm_of_the_second,
                TOLD_TO_THE_SECOND_DESK
            )
            .await,
            [
                OF_THE_SECOND_DESK.to_string(),
                TOLD_TO_THE_SECOND_DESK.to_string()
            ],
            "what the second mob's agent remembered in its turn is in the second mob's store, \
             beside its own earlier note and nothing of the launch mob's agent"
        );
    }
    assert_eq!(
        recalled_once(
            &kept.of_the_launch,
            &kept.config.realm,
            TOLD_TO_THE_LAUNCH_DESK
        )
        .await,
        [
            OF_THE_LAUNCH_DESK.to_string(),
            TOLD_TO_THE_LAUNCH_DESK.to_string()
        ],
        "what the launch mob's agent remembered in its turn is in the launch's store, beside \
         its own earlier note and nothing of the second mob's agent"
    );
    assert!(
        recalled(&kept.of_the_launch, &realm_of_the_second)
            .await
            .is_empty(),
        "the launch's store holds nothing under the second mob's realm"
    );

    for (told, own, of_the_other) in [
        (
            TOLD_TO_THE_LAUNCH_DESK,
            OF_THE_LAUNCH_DESK,
            OF_THE_SECOND_DESK,
        ),
        (
            TOLD_TO_THE_SECOND_DESK,
            OF_THE_SECOND_DESK,
            OF_THE_LAUNCH_DESK,
        ),
    ] {
        let requests = model.asked_with(&remember_ask(told));
        assert!(
            requests.iter().any(|request| request.contains(own)),
            "the model of the agent told {told:?} was sent that agent's own note for its \
             recall: {} requests",
            requests.len()
        );
        assert!(
            requests
                .iter()
                .all(|request| !request.contains(of_the_other)),
            "and no note of the other mob's agent of the same name"
        );
    }
    kept.shut_down().await;
}

/// A launch whose distiller and steward are on, and which hands its memory
/// over WITHOUT what the engines of its other mobs are built with, runs
/// none for a mob it hosts beside its launch mob (a launch that hands that
/// over gives each such mob engines of its own: the rows of
/// `mob_memory_engines_tests`). The launch mob's memory delivers with the
/// launch's engines; the second mob keeps agent memory and its memory
/// delivers with no distiller and no steward: what this launch hands over
/// for its other mobs carries no engine, no promotion gate and no conflict
/// bridge at all. Composing the second mob's memory registers no gating
/// observer with the host, and the launch's memory root holds the launch
/// mob's realm alone: the launch's engines hold the launch's own store.
#[tokio::test]
async fn a_launch_that_hands_over_no_engines_runs_none_for_a_hosted_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let engines = crate::memory_wiring::MemoryEnginesConfig {
        distiller: crate::memory::distiller::DistillerConfig {
            enabled: true,
            ..Default::default()
        },
        steward: crate::memory::steward::StewardConfig {
            enabled: true,
            ..Default::default()
        },
    };
    let launched =
        launch_with_memory_and_engines(&temp, config_of_the_launch(), &engines, |spec| spec).await;
    let observers_of_the_launch = launched.host.gating_resolution_observers_in_tests().await;
    let kept = with_a_second_mob(launched).await;

    let of_the_launch = kept
        .launch_context
        .runtime
        .agent_memory_in_tests()
        .await
        .map(|memory| memory.engines_in_tests());
    assert_eq!(
        of_the_launch,
        Some((true, true)),
        "the launch mob's memory delivers with the launch's distiller and steward"
    );

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let of_the_second = second_context
            .runtime
            .agent_memory_in_tests()
            .await
            .map(|memory| memory.engines_in_tests());
        assert_eq!(
            of_the_second,
            Some((false, false)),
            "the second mob keeps agent memory, and it delivers with no distiller and no steward"
        );
        // The second mob's agent is built with its memory all the same.
        let draft = built_desk(&second_context, kept.second_handle().await).await;
        let (answer, failed) =
            call_memory_tool(&draft, serde_json::json!({"action": "recall"})).await;
        assert!(
            !failed,
            "the second mob's agent has its `memory` tool: {answer}"
        );
    }
    assert_eq!(
        kept.host.gating_resolution_observers_in_tests().await,
        observers_of_the_launch,
        "composing the second mob's memory registered no gating observer with the host"
    );
    assert_eq!(
        kept.of_the_launch
            .known_realms()
            .expect("the launch's realms"),
        std::slice::from_ref(&kept.config.realm),
        "the launch's memory root, which its engines hold, has the launch mob's realm alone"
    );
    kept.shut_down().await;
}
