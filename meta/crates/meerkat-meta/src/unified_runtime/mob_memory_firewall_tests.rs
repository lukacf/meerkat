//! Rows of the two memory slots of a launch that hosts a mob beside its
//! launch mob: what observes a member's model calls for a memory's write
//! gate, and what binds the `memory` tool of a side chat and of a member
//! private to a person.
//!
//! The fixture is the launch of the per-mob memory rows, composed as a
//! gateway composes it: the launch's two slots are the ones its own spec
//! carries, filled from the launch's memory once it stands. Each row passes
//! a member's build through those slots, as every member create of the
//! launch passes them, under the mob binding the mob set writes on a build,
//! and reads what the member was given and where it wrote.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use meerkat_core::service::CreateSessionRequest;
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, Message, StopReason, ToolDef, ToolResult,
    Usage,
};
use meerkat_core::{AgentError, AgentLlmClient, LlmStreamResult, ProviderParamsOverride};
use meerkat_mob::{AgentIdentity, MobId, SpawnMemberSpec};

use super::mob_entries_tests::{set_of, shut_down};
use super::mob_memory_tests::{
    DESK, KeptMemory, LaunchWithMemory, built_desk, call_memory_tool, config_of_the_launch,
    definition_keeping_memory, launch_with_memory, with_a_second_mob,
};
use crate::identity_first::agent_memory::MEMORY_TOOL_NAME;
use crate::memory::capabilities::{EvidenceRefResolver as _, MemoryPanelStore as _};
use crate::memory::dispatch_taint::DispatchTaintSlot;
use crate::memory::distiller::TranscriptSource;
use crate::memory::records::{EvidenceRef, MemoryScope};
use crate::memory::side_chat_memory::SideChatMemorySlot;
use crate::memory::sqlite_store::SqliteAgentMemoryStore;
use crate::memory::steward::SessionStoreEvidenceResolver;

/// The name of the rows' side chats: one name, in whichever mob.
pub(super) const SIDE: &str = "desk-side";

/// The person the rows' private members are private to.
pub(super) const OWNER: &str = "console-caller:v1:aa";

/// A tool whose result is content from outside the host.
const TAKES_IN: &str = "fetch";

/// The two memory slots of a launch's own spec.
pub(super) struct Slots {
    pub(super) taint: DispatchTaintSlot,
    pub(super) side_chat: SideChatMemorySlot,
}

impl Slots {
    /// The two memory slots of the launch `host` was built from: the ones
    /// every member build of its mobs passes.
    pub(super) fn of(host: &super::UnifiedRuntime) -> Self {
        Self {
            taint: host.mob_runtime().dispatch_taint_slot(),
            side_chat: host.mob_runtime().side_chat_memory_slot(),
        }
    }
}

/// A launch that keeps agent memory, composed as a gateway composes it: the
/// two memory slots of its spec are filled from the launch's memory, the
/// one the launch's store answers its write gate from.
async fn launched_as_a_gateway(temp: &tempfile::TempDir) -> (LaunchWithMemory, Slots) {
    let launched = launch_with_memory(temp, config_of_the_launch(), |spec| spec).await;
    let slots = Slots::of(&launched.host);
    slots.taint.fill(launched.taint_of_the_launch.clone());
    slots.side_chat.fill(
        Arc::clone(&launched.provider_of_the_launch),
        launched.config.clone(),
    );
    (launched, slots)
}

/// [`launched_as_a_gateway`] with a second mob of the host, whose roster
/// holds an agent `desk` as the launch mob's does.
async fn two_mobs_as_a_gateway(temp: &tempfile::TempDir) -> (KeptMemory, Slots) {
    let (launched, slots) = launched_as_a_gateway(temp).await;
    (with_a_second_mob(launched).await, slots)
}

/// The create of the member `name` of the mob `mob_id`, as the mob set
/// builds a member: its mob binding names the mob and the member.
fn member_create(mob_id: &MobId, name: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "gpt-5.5".to_string(),
        prompt: meerkat_core::ContentInput::Text("hi".to_string()),
        injected_context: Vec::new(),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
        build: Some(meerkat_core::service::SessionBuildOptions {
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: mob_id.to_string(),
                role: "worker".to_string(),
                member: name.to_string(),
            }),
            ..meerkat_core::service::SessionBuildOptions::default()
        }),
        labels: None,
    }
}

/// A model that answers every call.
struct Answers;

#[async_trait::async_trait]
impl AgentLlmClient for Answers {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "noted".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    fn model(&self) -> &'static str {
        "gpt-5.5"
    }

    // A model double, not a decorator: it states the trait's default.
    fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
        meerkat_core::RequestAttemptAuthority::LegacySplit
    }
}

/// The member `name` of the mob `mob_id` makes a model call that carries a
/// result of [`TAKES_IN`], through the client its build was given by the
/// launch's slot. A build the slot gave no client of its own makes the
/// call on the bare model.
pub(super) async fn takes_in_outside_content(slots: &Slots, mob_id: &MobId, name: &str) {
    let mut create = member_create(mob_id, name);
    crate::memory::dispatch_taint::attach_member_taint_decorator(&mut create, &slots.taint);
    a_model_call_that_takes_in_outside_content(create).await;
}

/// One model call that carries a result of [`TAKES_IN`], through the client
/// the build of `create` was given. A build that was given no client of its
/// own makes the call on the bare model.
pub(super) async fn a_model_call_that_takes_in_outside_content(create: CreateSessionRequest) {
    let model: Arc<dyn AgentLlmClient> = Arc::new(Answers);
    let client = match create
        .build
        .and_then(|build| build.agent_llm_client_decorator)
    {
        Some(decorate) => decorate(model),
        None => model,
    };
    let conversation = [
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: "call-takes-in".to_string(),
                name: TAKES_IN.to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string())
                    .expect("tool arguments"),
                meta: None,
            }],
            StopReason::ToolUse,
        )),
        Message::tool_results(vec![ToolResult {
            host_metadata: Default::default(),
            settlement_failures: Default::default(),
            tool_use_id: "call-takes-in".to_string(),
            content: vec![ContentBlock::Text {
                text: "a page from outside".to_string(),
            }],
            is_error: false,
        }]),
    ];
    client
        .stream_response(&conversation, &[], 128, None, None)
        .await
        .expect("the model answers");
}

/// Per-spawn tools that hold a `memory` tool of another build, as a fork
/// carries its source's: it counts the calls that reach it.
#[derive(Default)]
pub(super) struct AnothersMemory {
    reached: AtomicUsize,
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for AnothersMemory {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            MEMORY_TOOL_NAME,
            "The memory of the build these tools were made for.",
            serde_json::json!({"type": "object"}),
        ))]
        .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.reached.fetch_add(1, Ordering::SeqCst);
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            ToolResult::new(call.id.to_string(), "another build's".to_string(), false),
        ))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

/// What the launch's slot made of the build of a side chat.
pub(super) struct SideChat {
    /// The per-spawn tools the build hands its member.
    tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    /// The tools the build carried before the slot.
    carried: Arc<AnothersMemory>,
}

impl SideChat {
    /// Whether the member is offered a `memory` tool.
    pub(super) fn is_offered_memory(&self) -> bool {
        self.tools.as_ref().is_some_and(|tools| {
            tools
                .tools()
                .iter()
                .any(|tool| tool.name.as_ref() == MEMORY_TOOL_NAME)
        })
    }

    /// One `remember` of the member's `memory` tool, titled `title`: `Ok`
    /// with whether the tool answered an error, `Err` when no tool of the
    /// member served the call.
    pub(super) async fn remembers(&self, title: &str) -> Result<bool, String> {
        let tools = self.tools.as_ref().ok_or("the build holds no tools")?;
        let args = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "action": "remember",
                "title": title,
                "body": format!("{title}, in full."),
            })
            .to_string(),
        )
        .expect("tool arguments");
        tools
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-remember",
                name: MEMORY_TOOL_NAME,
                args: &args,
            })
            .await
            .map(|outcome| outcome.result.is_error)
            .map_err(|error| error.to_string())
    }

    /// One `recall` of the member's `memory` tool, with no query: `Ok` with
    /// the answer's text, `Err` when no tool of the member served the call
    /// or the tool answered an error, with what it said.
    pub(super) async fn recalls(&self) -> Result<String, String> {
        let tools = self.tools.as_ref().ok_or("the build holds no tools")?;
        let args = serde_json::value::RawValue::from_string(
            serde_json::json!({ "action": "recall" }).to_string(),
        )
        .expect("tool arguments");
        let outcome = tools
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-recall",
                name: MEMORY_TOOL_NAME,
                args: &args,
            })
            .await
            .map_err(|error| error.to_string())?;
        let text = outcome
            .result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if outcome.result.is_error {
            Err(text)
        } else {
            Ok(text)
        }
    }

    /// How many calls reached the `memory` tool the build carried.
    pub(super) fn reached_the_carried_tool(&self) -> usize {
        self.carried.reached.load(Ordering::SeqCst)
    }
}

/// The build of the side chat `name` of the member `source` in the mob
/// `mob_id`, private to `owner` when one is given, with per-spawn tools
/// that hold a `memory` tool of another build, after the launch's slot.
pub(super) fn side_chat(
    slots: &Slots,
    mob_id: &MobId,
    name: &str,
    source: &str,
    owner: Option<&str>,
) -> SideChat {
    let carried = Arc::new(AnothersMemory::default());
    let mut create = member_create(mob_id, name);
    let mut labels = BTreeMap::from([(
        crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
        source.to_string(),
    )]);
    if let Some(owner) = owner {
        labels.insert(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            owner.to_string(),
        );
    }
    create.labels = Some(labels);
    if let Some(build) = create.build.as_mut() {
        build.external_tools =
            Some(Arc::clone(&carried) as Arc<dyn meerkat_core::AgentToolDispatcher>);
    }
    slots.side_chat.bind_side_chat_memory(&mut create);
    SideChat {
        tools: create.build.and_then(|build| build.external_tools),
        carried,
    }
}

/// What a store holds in one realm: per record the key of its scope, its
/// title and its status.
pub(super) type Stored = Vec<(String, String, &'static str)>;

/// Every record `store` holds in `realm`, sorted.
pub(super) async fn stored(store: &SqliteAgentMemoryStore, realm: &str) -> Stored {
    let mut records = store
        .records_page(realm, None, None, None, 100, None)
        .await
        .expect("the store lists its records")
        .records
        .into_iter()
        .map(|record| {
            let key = match &record.scope {
                MemoryScope::Identity { identity, .. } => identity.clone(),
                other => format!("{other:?}"),
            };
            (key, record.title, record.status.kind_str())
        })
        .collect::<Vec<_>>();
    records.sort();
    records
}

/// Seat a member `maker` in the mob `mob_id` serves and have the mob set
/// create a mob for that member's session: a mob a member of `mob_id`
/// created.
async fn a_mob_a_member_creates(kept_host: &super::UnifiedRuntime, mob_id: &MobId) -> MobId {
    a_mob_created_by(
        kept_host,
        mob_id,
        "maker",
        definition_keeping_memory("created"),
    )
    .await
}

/// Seat a member `maker` in the mob `mob_id` serves and have the mob set
/// create the mob of `definition` for that member's session.
pub(super) async fn a_mob_created_by(
    kept_host: &super::UnifiedRuntime,
    mob_id: &MobId,
    maker: &str,
    definition: meerkat_mob::MobDefinition,
) -> MobId {
    let handle = set_of(kept_host)
        .handle_for(mob_id)
        .await
        .expect("the set serves the creating mob");
    let seated = Box::pin(handle.spawn_spec(SpawnMemberSpec::new("worker", maker))).await;
    assert!(
        seated.is_ok(),
        "the creating member: {:?}",
        seated.as_ref().err()
    );
    let session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(maker))
        .await
        .expect("the creating member's session");
    Box::pin(
        set_of(kept_host)
            .mob_create_definition_with_owner_bridge_session(definition, session, true, false),
    )
    .await
    .expect("a mob the member creates")
}

/// Have the mob set create a mob for a session that is no member's.
async fn a_mob_no_member_creates(kept_host: &super::UnifiedRuntime) -> MobId {
    Box::pin(
        set_of(kept_host).mob_create_definition_with_owner_bridge_session(
            definition_keeping_memory("unowned"),
            meerkat_core::types::SessionId::new(),
            true,
            false,
        ),
    )
    .await
    .expect("a mob created for a session that is no member's")
}

fn remember(title: &str) -> serde_json::Value {
    serde_json::json!({
        "action": "remember",
        "title": title,
        "body": format!("{title}, in full."),
    })
}

/// What an agent of a hosted mob takes in from outside marks that agent in
/// its own mob's memory, and nobody in the launch's: the launch's memory
/// holds no mark under the agent's name, the hosted mob's agent's next
/// write is set aside for review, and the launch mob's agent of the same
/// name writes as before.
#[tokio::test]
async fn what_a_hosted_mobs_agent_takes_in_marks_it_in_its_own_mobs_memory_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;

    takes_in_outside_content(&slots, &kept.second_id, DESK).await;
    assert!(
        kept.taint_of_the_launch.identity_taint(DESK).is_none(),
        "what a hosted mob's agent took in marks nobody in the launch's memory"
    );

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let draft = built_desk(&second_context, kept.second_handle().await).await;
        let (answer, failed) =
            call_memory_tool(&draft, remember("The second desk heard a rumour")).await;
        assert!(
            !failed && answer.contains("QUARANTINED"),
            "the hosted mob's agent's write after what it took in is set aside: {answer}"
        );
    }

    let launch_draft = built_desk(
        &kept.launch_context,
        kept.host.mob_runtime().handle().observer(),
    )
    .await;
    let (answer, failed) =
        call_memory_tool(&launch_draft, remember("The launch desk opens at eight")).await;
    assert!(
        !failed && !answer.contains("QUARANTINED"),
        "the launch mob's agent of the same name writes as before: {answer}"
    );
    kept.shut_down().await;
}

/// A write of a marked agent of a hosted mob is set aside in its own mob's
/// store, in its own mob's realm, and nowhere else: the launch's store
/// holds no record of it.
#[tokio::test]
async fn a_write_of_a_marked_agent_of_a_hosted_mob_is_set_aside_in_its_own_mobs_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;
    takes_in_outside_content(&slots, &kept.second_id, DESK).await;

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of a launch that keeps agent memory has an identity plane"
    );
    if let Some(second_context) = second_context {
        let draft = built_desk(&second_context, kept.second_handle().await).await;
        let (answer, failed) =
            call_memory_tool(&draft, remember("The second desk heard a rumour")).await;
        assert!(
            !failed,
            "the hosted mob's agent's write is stored: {answer}"
        );
    }
    assert_eq!(
        stored(&kept.of_the_second(), &kept.realm_of_the_second()).await,
        [(
            DESK.to_string(),
            "The second desk heard a rumour".to_string(),
            "quarantined"
        )],
        "the write is set aside in the hosted mob's own store, under the agent's name"
    );
    assert_eq!(
        stored(&kept.of_the_launch, &kept.config.realm).await,
        Stored::new(),
        "the launch's store holds no record of it"
    );
    kept.shut_down().await;
}

/// A side chat of a hosted mob's agent keeps its notes in that mob's store,
/// under its own name; a side chat of the same name of the launch mob's
/// agent of the same name keeps its notes in the launch's. Neither store
/// holds the other's, and the `memory` tool each build carried is never
/// reached.
#[tokio::test]
async fn a_side_chat_of_a_hosted_mobs_agent_keeps_its_notes_in_that_mobs_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();

    let of_the_second = side_chat(&slots, &kept.second_id, SIDE, DESK, None);
    assert!(
        of_the_second.is_offered_memory(),
        "a side chat of a hosted mob's agent is offered a `memory` tool"
    );
    assert_eq!(
        of_the_second
            .remembers("A note of the second mob's side chat")
            .await,
        Ok(false),
        "the hosted mob's side chat stores its note"
    );
    let of_the_launch = side_chat(&slots, &launch_mob, SIDE, DESK, None);
    assert_eq!(
        of_the_launch
            .remembers("A note of the launch mob's side chat")
            .await,
        Ok(false),
        "the launch mob's side chat stores its note"
    );
    let recalled = of_the_second.recalls().await;
    assert!(
        recalled.as_ref().is_ok_and(|text| {
            text.contains("A note of the second mob's side chat")
                && !text.contains("A note of the launch mob's side chat")
        }),
        "the hosted mob's side chat recalls its own note and not the launch mob's: {recalled:?}"
    );

    assert_eq!(
        stored(&kept.of_the_second(), &kept.realm_of_the_second()).await,
        [(
            SIDE.to_string(),
            "A note of the second mob's side chat".to_string(),
            "active"
        )],
        "the hosted mob's side chat keeps its note in its own mob's store, under its own name"
    );
    assert_eq!(
        stored(&kept.of_the_launch, &kept.config.realm).await,
        [(
            SIDE.to_string(),
            "A note of the launch mob's side chat".to_string(),
            "active"
        )],
        "the launch's store holds the launch mob's side chat's note and no other"
    );
    assert_eq!(
        (
            of_the_second.reached_the_carried_tool(),
            of_the_launch.reached_the_carried_tool()
        ),
        (0, 0),
        "the `memory` tool a side chat's build carried is never reached"
    );
    kept.shut_down().await;
}

/// A side chat of a hosted mob's agent that is private to a person keeps
/// its notes in that mob's store, under its owner and its name; the
/// launch's store holds none of them.
#[tokio::test]
async fn a_private_member_of_a_hosted_mob_keeps_its_notes_in_that_mobs_store_under_its_owner() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;

    let private = side_chat(&slots, &kept.second_id, SIDE, DESK, Some(OWNER));
    assert_eq!(
        private.remembers("A note one person asked for").await,
        Ok(false),
        "the private side chat stores its note"
    );
    assert_eq!(
        stored(&kept.of_the_second(), &kept.realm_of_the_second()).await,
        [(
            crate::memory::records::private_member_scope_key(OWNER, SIDE),
            "A note one person asked for".to_string(),
            "active"
        )],
        "the note is kept in the hosted mob's own store, under its owner and its name"
    );
    assert_eq!(
        stored(&kept.of_the_launch, &kept.config.realm).await,
        Stored::new(),
        "the launch's store holds none of it"
    );
    kept.shut_down().await;
}

/// A member of a mob that a hosted mob's member created gets no memory work
/// of the launch mob: what it takes in marks nobody in the launch's memory,
/// also under the name of a launch mob's agent, and a side chat seated in
/// that mob is offered no `memory` tool, reaches none and writes nothing.
#[tokio::test]
async fn a_member_of_a_mob_that_a_hosted_mobs_member_created_gets_no_memory_work_of_the_launch_mob()
{
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;
    let created = a_mob_a_member_creates(&kept.host, &kept.second_id).await;

    takes_in_outside_content(&slots, &created, DESK).await;
    assert!(
        kept.taint_of_the_launch.identity_taint(DESK).is_none(),
        "what a member of that mob took in marks nobody in the launch's memory"
    );

    let seated = side_chat(&slots, &created, SIDE, DESK, None);
    assert!(
        !seated.is_offered_memory(),
        "a side chat seated in that mob is offered no `memory` tool"
    );
    assert!(
        seated
            .remembers("A note of a mob with no memory")
            .await
            .is_err(),
        "a call of `memory` there is served by no tool"
    );
    assert_eq!(
        seated.reached_the_carried_tool(),
        0,
        "the `memory` tool the build carried is never reached"
    );
    assert_eq!(
        (
            stored(&kept.of_the_launch, &kept.config.realm).await,
            stored(&kept.of_the_second(), &kept.realm_of_the_second()).await
        ),
        (Vec::new(), Vec::new()),
        "neither the launch's store nor the hosted mob's holds a record of it"
    );
    shut_down(&kept.host, &[&created, &kept.second_id]).await;
}

/// A mob whose creation names no member is in no mob's memory by its
/// records: a side chat seated in it is offered no `memory` tool, reaches
/// none and writes nothing.
#[tokio::test]
async fn a_member_of_a_mob_whose_creation_names_no_member_is_offered_no_memory_tool() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;
    let unowned = a_mob_no_member_creates(&kept.host).await;

    let seated = side_chat(&slots, &unowned, SIDE, DESK, None);
    assert!(
        !seated.is_offered_memory(),
        "a side chat seated in that mob is offered no `memory` tool"
    );
    assert!(
        seated
            .remembers("A note of a mob with no memory")
            .await
            .is_err(),
        "a call of `memory` there is served by no tool"
    );
    assert_eq!(
        seated.reached_the_carried_tool(),
        0,
        "the `memory` tool the build carried is never reached"
    );
    assert_eq!(
        stored(&kept.of_the_launch, &kept.config.realm).await,
        Stored::new(),
        "the launch's store holds no record of it"
    );
    shut_down(&kept.host, &[&unowned, &kept.second_id]).await;
}

/// A guard. A side chat of the launch mob's agent keeps its notes where it
/// kept them: in the launch's store, in the launch's realm, under its own
/// name, on a host of one mob and on a host that hosts a second mob.
#[tokio::test]
async fn a_side_chat_of_the_launch_mobs_agent_keeps_its_notes_where_it_kept_them() {
    for hosts_a_second_mob in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let (launched, slots) = launched_as_a_gateway(&temp).await;
        let kept = if hosts_a_second_mob {
            Ok(with_a_second_mob(launched).await)
        } else {
            Err(launched)
        };
        let (host, of_the_launch, realm) = match &kept {
            Ok(kept) => (&kept.host, &kept.of_the_launch, &kept.config.realm),
            Err(launched) => (
                &launched.host,
                &launched.of_the_launch,
                &launched.config.realm,
            ),
        };
        let launch_mob = host.mob_handle().mob_id().clone();

        let of_the_launch_mob = side_chat(&slots, &launch_mob, SIDE, DESK, None);
        assert!(
            of_the_launch_mob.is_offered_memory(),
            "a side chat of the launch mob's agent is offered a `memory` tool \
             (a second mob: {hosts_a_second_mob})"
        );
        assert_eq!(
            of_the_launch_mob
                .remembers("A note of the launch mob's side chat")
                .await,
            Ok(false),
            "the launch mob's side chat stores its note (a second mob: {hosts_a_second_mob})"
        );
        assert_eq!(
            stored(of_the_launch, realm).await,
            [(
                SIDE.to_string(),
                "A note of the launch mob's side chat".to_string(),
                "active"
            )],
            "the note is kept in the launch's store, under the side chat's own name \
             (a second mob: {hosts_a_second_mob})"
        );
        match kept {
            Ok(kept) => kept.shut_down().await,
            Err(launched) => shut_down(&launched.host, &[]).await,
        }
    }
}

/// A guard. A member of a mob the launch mob's member created takes part
/// in the launch's memory as before: what it takes in marks it in the
/// launch's memory, on a host that hosts a second mob too.
#[tokio::test]
async fn a_member_of_a_mob_the_launch_mobs_member_created_takes_part_as_before() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let created = a_mob_a_member_creates(&kept.host, &launch_mob).await;

    takes_in_outside_content(&slots, &created, "helper").await;
    assert!(
        kept.taint_of_the_launch.identity_taint("helper").is_some(),
        "what a member of a mob the launch mob's member created took in marks it in the \
         launch's memory"
    );
    shut_down(&kept.host, &[&created, &kept.second_id]).await;
}

/// A guard. A member of a mob whose creation names no member is observed
/// as before: what it takes in marks it in the launch's memory, so no
/// model call of a member goes unobserved because its mob's records do
/// not say whose memory it is in.
#[tokio::test]
async fn a_member_of_a_mob_whose_creation_names_no_member_is_observed_as_before() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;
    let unowned = a_mob_no_member_creates(&kept.host).await;

    takes_in_outside_content(&slots, &unowned, "helper").await;
    assert!(
        kept.taint_of_the_launch.identity_taint("helper").is_some(),
        "what a member of that mob took in marks it in the launch's memory"
    );
    shut_down(&kept.host, &[&unowned, &kept.second_id]).await;
}

/// A session stored in `sessions`, bound to the member `member` of the mob
/// `mob` when `bound_to` gives them: its key.
pub(super) async fn a_session(
    sessions: &Arc<meerkat::MemoryStore>,
    bound_to: Option<(&MobId, &str)>,
) -> String {
    crate::memory_wiring::a_stored_member_session_in_tests(
        sessions,
        bound_to.map(|(mob, member)| (mob.as_str(), member)),
    )
    .await
}

/// [`a_session`] stored under `id`, the id a mob's journal bound.
pub(super) async fn a_session_stored_under(
    sessions: &Arc<meerkat::MemoryStore>,
    id: meerkat_core::types::SessionId,
    bound_to: Option<(&MobId, &str)>,
) -> String {
    crate::memory_wiring::a_stored_session_in_tests(
        sessions,
        Some(id),
        bound_to.map(|(mob, member)| (mob.as_str(), member)),
        true,
    )
    .await
}

/// Seat the member `member` in the mob `mob` the set serves: the session
/// that mob's own journal bound it to.
pub(super) async fn a_member_seated_in(
    host: &super::UnifiedRuntime,
    mob: &MobId,
    member: &str,
) -> meerkat_core::types::SessionId {
    let handle = set_of(host)
        .handle_for(mob)
        .await
        .expect("the set serves the mob");
    let seated = Box::pin(handle.spawn_spec(SpawnMemberSpec::new("worker", member))).await;
    assert!(
        seated.is_ok(),
        "the mob's member: {:?}",
        seated.as_ref().err()
    );
    handle
        .last_member_binding(&AgentIdentity::from(member))
        .await
        .expect("the mob's journal reads")
        .and_then(|binding| binding.session_id().cloned())
        .expect("the mob's journal bound the member to a session")
}

/// What a transcript source answers for one stored session: whether a
/// read holds it, whether it is private to a person (`None` when the
/// source says that is unknown), and whether an evidence reference to
/// its first message resolves on that source.
pub(super) async fn answered(
    source: &Arc<dyn TranscriptSource>,
    session: &str,
) -> (bool, Option<bool>, bool) {
    let read = source
        .read(session, 0)
        .await
        .expect("the source reads")
        .is_some();
    let private = source.private_to_a_person(session).await.ok();
    let resolver =
        SessionStoreEvidenceResolver::new(Arc::clone(source), tokio::runtime::Handle::current());
    let evidence = EvidenceRef {
        session_id: session.to_string(),
        generation: 0,
        revision: None,
        range: Some((0, 0)),
    };
    let resolves = tokio::task::spawn_blocking(move || resolver.resolves(&evidence).is_ok())
        .await
        .expect("the resolver answers");
    (read, private, resolves)
}

/// A session the source holds: read, private as it is stored, resolved.
pub(super) const HELD: (bool, Option<bool>, bool) = (true, Some(true), true);

/// A session the source does not hold: it reads as a session its store
/// has none of, whether it is private to a person is unknown, and no
/// reference to it resolves.
pub(super) const NOT_HELD: (bool, Option<bool>, bool) = (false, None, false);

/// The launch mob's memory engines read the transcripts of the sessions of
/// the launch's memory, and of no other mob's: asked of the source a
/// launch assembles its engines with, a session of a member of the launch
/// mob and of a member of a mob a launch mob's member created is held when
/// that mob's own journal bound it; a session of no member, a session of
/// the second mob's member and of a member of a mob the second mob's
/// member created (each bound by its own mob's journal), and a session
/// that names an id which the mob that holds the id now never bound, read
/// as sessions the store does not hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_launch_mobs_memory_engines_read_the_transcripts_of_sessions_whose_mob_has_its_memory_home_at_the_launch()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, _slots) = two_mobs_as_a_gateway(&temp).await;
    let launch_mob = kept.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());

    // A session that names an id before the mob that will hold it exists:
    // a session of an earlier mob of that id, which the mob that holds the
    // id now never bound.
    let later = definition_keeping_memory("later");
    let later_id = later.id.clone();
    let of_an_earlier_mob = a_session(&sessions, Some((&later_id, "helper"))).await;

    let created = a_mob_a_member_creates(&kept.host, &launch_mob).await;
    let later = a_mob_created_by(&kept.host, &launch_mob, "second-maker", later).await;
    assert_eq!(later, later_id);
    let created_in_the_second = a_mob_a_member_creates(&kept.host, &kept.second_id).await;
    let owners = kept.host.mob_runtime().private_owners().clone();
    let read = owners.read_loaded_mobs().await;
    assert!(
        read.is_ok(),
        "the record reads the mobs created since: {read:?}"
    );

    // A member seated in each mob, and its session stored under the id
    // that mob's journal bound it to.
    let mut of_a_member = Vec::new();
    for mob in [
        &launch_mob,
        &created,
        &later,
        &kept.second_id,
        &created_in_the_second,
    ] {
        let bound = a_member_seated_in(&kept.host, mob, "helper").await;
        of_a_member.push(a_session_stored_under(&sessions, bound, Some((mob, "helper"))).await);
    }
    let [
        of_the_launch_mob,
        of_the_created_mob,
        of_the_later_mob,
        of_the_second_mob,
        of_a_mob_of_the_second,
    ] = <[String; 5]>::try_from(of_a_member).expect("five sessions");

    let table = [
        (
            of_the_launch_mob,
            HELD,
            "a session of a member of the launch mob",
        ),
        (
            of_the_created_mob,
            HELD,
            "a session of a member of a mob a launch mob's member created",
        ),
        (
            of_the_later_mob,
            HELD,
            "a session of a member of the mob that holds an id now",
        ),
        (
            a_session(&sessions, None).await,
            NOT_HELD,
            "a session of no member",
        ),
        (
            of_the_second_mob,
            NOT_HELD,
            "a session of the second mob's member, which that mob's journal bound",
        ),
        (
            of_a_mob_of_the_second,
            NOT_HELD,
            "a session of a member of a mob the second mob's member created",
        ),
        (
            of_an_earlier_mob,
            NOT_HELD,
            "a session that names an id which the mob that holds it now never bound",
        ),
    ];
    let source =
        crate::memory_wiring::launch_transcript_source(sessions, owners, kept.host.mob_handle());
    for (session, expected, what) in table {
        assert_eq!(answered(&source, &session).await, expected, "{what}");
    }
    shut_down(
        &kept.host,
        &[&created, &later, &created_in_the_second, &kept.second_id],
    )
    .await;
}

/// On a host whose only host mob is the launch mob, the engines read their
/// members' sessions as before: a session of a member of the launch mob
/// and of a member of a mob a launch mob's member created, each bound by
/// its mob's journal. A session of no member is not held: no journal binds
/// it. A session of a member of a mob no member created is not held
/// although that mob's journal bound it: that mob's records do not say
/// whose memory its members are in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_a_host_whose_only_host_mob_is_the_launch_mob_the_engines_read_their_members_sessions_as_before_and_none_of_a_mob_no_member_created()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let (launched, _slots) = launched_as_a_gateway(&temp).await;
    let launch_mob = launched.host.mob_handle().mob_id().clone();
    let sessions = Arc::new(meerkat::MemoryStore::new());
    let created = a_mob_a_member_creates(&launched.host, &launch_mob).await;
    let unowned = a_mob_no_member_creates(&launched.host).await;
    let owners = launched.host.mob_runtime().private_owners().clone();
    let read = owners.read_loaded_mobs().await;
    assert!(
        read.is_ok(),
        "the record reads the mobs created since: {read:?}"
    );

    let mut of_a_member = Vec::new();
    for mob in [&launch_mob, &created, &unowned] {
        let bound = a_member_seated_in(&launched.host, mob, "helper").await;
        of_a_member.push(a_session_stored_under(&sessions, bound, Some((mob, "helper"))).await);
    }
    let [of_the_launch_mob, of_the_created_mob, of_the_unowned_mob] =
        <[String; 3]>::try_from(of_a_member).expect("three sessions");

    let table = [
        (
            of_the_launch_mob,
            HELD,
            "a session of a member of the launch mob",
        ),
        (
            of_the_created_mob,
            HELD,
            "a session of a member of a mob a launch mob's member created",
        ),
        (
            a_session(&sessions, None).await,
            NOT_HELD,
            "a session of no member",
        ),
        (
            of_the_unowned_mob,
            NOT_HELD,
            "a session of a member of a mob no member created",
        ),
    ];
    let source = crate::memory_wiring::launch_transcript_source(
        sessions,
        owners,
        launched.host.mob_handle(),
    );
    for (session, expected, what) in table {
        assert_eq!(answered(&source, &session).await, expected, "{what}");
    }
    shut_down(&launched.host, &[&created, &unowned]).await;
}

/// Every launch names its launch mob its own in its record of its mobs
/// before its memory engines read a session: the record is told where a
/// launch's spec makes it, before the launch is bootstrapped, and names no
/// other mob the launch's; and each launch asks the record for the home of
/// its own mob, and its mob's storage for the journal of its members'
/// sessions, before it makes the source its engines read through.
#[tokio::test]
async fn every_launch_binds_its_launch_mob_in_its_record_before_its_memory_engines_read() {
    use crate::private_owners::MemoryHome;

    let temp = tempfile::tempdir().expect("tempdir");
    let seen: Arc<Mutex<Option<(MemoryHome, MemoryHome)>>> = Arc::default();
    let of_the_spec = Arc::clone(&seen);
    let launched = launch_with_memory(&temp, config_of_the_launch(), move |spec| {
        *of_the_spec.lock().unwrap_or_else(PoisonError::into_inner) = Some((
            spec.private_owners
                .memory_home_of(spec.definition.id.as_str()),
            spec.private_owners.memory_home_of("another-mob"),
        ));
        spec
    })
    .await;
    assert_eq!(
        seen.lock().unwrap_or_else(PoisonError::into_inner).take(),
        Some((MemoryHome::Launch, MemoryHome::NotKnown)),
        "a launch's spec names its launch mob its own, and no other mob, before the bootstrap"
    );
    let launch_mob = launched.host.mob_handle().mob_id().clone();
    assert_eq!(
        crate::memory_wiring::launch_mob_is_bound(
            launched.host.mob_runtime().private_owners(),
            &launch_mob
        ),
        Ok(()),
        "what a launch asks before it assembles its engines"
    );

    // Both launches assemble their own stack through the one entry, over
    // the launch's source, and ask for the home of their own mob first.
    for (file, source) in [
        ("rpc_gateway.rs", include_str!("../rpc_gateway.rs")),
        ("unified_runtime/builder.rs", include_str!("builder.rs")),
    ] {
        let asked = source.find("memory_wiring::launch_mob_is_bound(");
        let journal = source.find("memory_wiring::launch_mob_keeps_a_journal(");
        let made = source.find("memory_wiring::launch_transcript_source(");
        let assembled = source.find("memory_wiring::assemble_memory_stack(");
        assert!(
            matches!(
                (asked, journal, made, assembled),
                (Some(asked), Some(journal), Some(made), Some(assembled))
                    if asked < journal && journal < made && made < assembled
            ),
            "{file}: the home of the launch's own mob, then the journal of its storage, then \
             the launch's source, then the assembly"
        );
        assert!(
            !source.contains("attach_memory_engines_with_provider_auth("),
            "{file} assembles no stack of its own over a source that reads every session"
        );
    }
    shut_down(&launched.host, &[]).await;
}

/// Wait, for at most ten seconds, until `holds`; whether it came to hold.
async fn comes_to_hold(holds: impl Fn() -> bool) -> bool {
    for _ in 0..200 {
        if holds() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    holds()
}

/// A mob created under the id of an earlier hosted mob starts without the
/// private retirements a memory of the earlier mob's recorded. The create's
/// admission drops what the record read of them: a name that only the
/// earlier mob's memory recorded as retired in private is private to no
/// person in the new mob on that word. A destroy alone drops nothing the
/// record read.
#[tokio::test]
async fn a_mob_created_under_a_hosted_mobs_id_starts_without_the_retirements_the_earlier_mobs_memory_recorded()
 {
    use crate::memory::capabilities::StewardStore;
    use crate::mob_announcements::MemberPerson;

    let temp = tempfile::tempdir().expect("tempdir");
    let (launched, _slots) = launched_as_a_gateway(&temp).await;
    let second = definition_keeping_memory("second");
    let second_id = second.id.clone();
    let created = super::mob_entries_tests::second_mob(&launched.host, second.clone()).await;
    assert_eq!(created, second_id);
    let composed = launched.host.resolve_mob(Some(&second_id)).await;
    assert!(composed.is_ok(), "the second mob resolves");

    let owners = launched.host.mob_runtime().private_owners().clone();
    let private_to_someone =
        MemberPerson::Private(crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
    let person = |name: &str| owners.person_of(second_id.as_str(), name);
    let the_mob_is_read = || owners.has_read_the_mob(second_id.as_str());
    assert!(
        comes_to_hold(the_mob_is_read).await,
        "the record comes to say it has read the second mob"
    );
    // A memory of the earlier mob's that a mob created under the id later
    // does not have: a store apart, bound as the mob's.
    let apart: Arc<dyn StewardStore> = Arc::new(
        launched
            .of_the_launch
            .of_hosted_mob_in_tests(&MobId::from("apart"))
            .expect("a store apart"),
    );
    let realm = format!("mob.{second_id}");
    let recorded = apart
        .record_private_retirement(&realm, "phantom", None, "retired", 5)
        .await;
    assert!(recorded.is_ok(), "the retirement is recorded: {recorded:?}");
    owners.bind_private_retirements_of(&second_id, Arc::clone(&apart), realm);
    assert!(
        comes_to_hold(|| person("phantom") == private_to_someone).await,
        "the record reads what the earlier mob's memory recorded"
    );

    let destroyed = Box::pin(set_of(&launched.host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert_eq!(
        person("phantom"),
        private_to_someone,
        "a destroy alone drops nothing the record read"
    );

    // A mob is created under the same id.
    let again = super::mob_entries_tests::second_mob(&launched.host, second).await;
    assert_eq!(again, second_id);
    let composed = launched.host.resolve_mob(Some(&second_id)).await;
    assert!(composed.is_ok(), "the mob created anew resolves");
    assert!(
        comes_to_hold(the_mob_is_read).await,
        "the record comes to say it has read the mob created anew"
    );
    assert_ne!(
        person("phantom"),
        private_to_someone,
        "what only the earlier mob's memory recorded names no member of the new mob"
    );
    // The store apart lived until here: its read held it weakly.
    drop(apart);
    shut_down(&launched.host, &[&second_id]).await;
}

/// A reading pass that noted the creates before a mob was created, and
/// lists the mobs after it was built, leaves that mob unread: it writes no
/// time for the mob's id, and the pass that follows reads the mob and
/// dates it. The record is one of the row's own that follows the launch's
/// mob set and runs no pass by itself (the launch's own record runs one
/// after each change of the set), and it is told the create's admission
/// by the row, as the launch's record is told by the set.
#[tokio::test]
async fn a_pass_that_noted_before_a_create_leaves_the_mob_created_since_for_the_next_pass() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (launched, _slots) = launched_as_a_gateway(&temp).await;
    let launch_mob = launched.host.mob_handle().mob_id().clone();
    let record = crate::private_owners::PrivateOwners::default();
    record.bind_launch_mob(launch_mob.as_str());
    record.bind_child_mobs(&set_of(&launched.host));
    let read = record.read_loaded_mobs().await;
    assert!(
        read.is_ok(),
        "the record reads the mobs the set manages: {read:?}"
    );

    let definition = definition_keeping_memory("created");
    let created_id = definition.id.clone();
    let read = record
        .read_loaded_mobs_noted_before(async {
            let created = a_mob_created_by(&launched.host, &launch_mob, "maker", definition).await;
            record.admit_a_create_in_tests(created.as_str(), launch_mob.as_str());
        })
        .await;
    assert!(
        read.is_ok(),
        "the pass that noted before the create: {read:?}"
    );
    assert_eq!(
        record.first_event_of(created_id.as_str()),
        None,
        "the pass that noted before the create writes no time for the mob created since"
    );

    let read = record.read_loaded_mobs().await;
    assert!(read.is_ok(), "the pass that follows: {read:?}");
    assert!(
        record.first_event_of(created_id.as_str()).is_some(),
        "the mob was left unread, so the pass that follows reads it and dates it"
    );
    assert_eq!(
        record.memory_home_of(created_id.as_str()),
        crate::private_owners::MemoryHome::Launch,
        "the mob created since is a mob of the launch's memory once the pass has read it"
    );
    shut_down(&launched.host, &[&created_id]).await;
}

/// The files below `dir` this process holds open now, read from the
/// process's own table of open files: a store value that is alive with a
/// database open shows here, whoever keeps it. On a platform without that
/// table it answers none.
fn open_below(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut open = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
            for entry in entries.filter_map(Result::ok) {
                if let Ok(target) = std::fs::read_link(entry.path())
                    && target.starts_with(&dir)
                {
                    open.push(target);
                }
            }
        }
        open.sort();
        open.dedup();
    }
    #[cfg(not(target_os = "linux"))]
    let _ = dir;
    open
}

/// The set destroys a hosted mob whose agent was built and kept a note,
/// and one of whose members took in content from outside. What served the
/// mob's memory while its
/// identity plane lived (the observer of its members' events, and the
/// handle its tracker declares a member's outbound taint through) ended
/// with the plane: nothing the launch keeps for the life of the process
/// holds the mob. The destroy leaves no cleanup outstanding, once the
/// mob's entry went the process holds no file of the mob's memory open,
/// and a create under the freed id is admitted.
///
/// This fixture's mob set unloads no idle mob, so the row ends the mob's
/// entry by a destroy; the end of an entry is the same at an unload.
#[tokio::test]
async fn a_hosted_mob_whose_agent_used_its_memory_is_destroyed_whole() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (kept, slots) = two_mobs_as_a_gateway(&temp).await;
    let memory_of_the_second = kept
        .of_the_launch
        .root_of_hosted_mob(&kept.second_id)
        .expect("the directory of the second mob's memory");
    let definition = kept.second_handle().await.definition().clone();
    {
        let second_context = kept
            .second_context()
            .await
            .expect("the second mob's identity plane");
        // A mark in the mob's tracker, first: the tracker declares the
        // member's outbound taint through the mob's handle, and that
        // declaration is answered while the agent below is built and
        // writes. The marked member is another one than the agent, whose
        // write is then not set aside.
        takes_in_outside_content(&slots, &kept.second_id, "courier").await;
        let draft = built_desk(&second_context, kept.second_handle().await).await;
        let (answer, failed) =
            call_memory_tool(&draft, remember("A note of the second mob's desk")).await;
        assert!(!failed, "the second mob's agent stores a note: {answer}");
    }
    #[cfg(target_os = "linux")]
    assert!(
        !open_below(&memory_of_the_second).is_empty(),
        "while the mob is loaded and its agent used its memory, its memory is open \
         (what the row reads below reads something)"
    );

    let destroyed = Box::pin(set_of(&kept.host).mob_destroy(&kept.second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the hosted mob whole: {:?}",
        destroyed.as_ref().err()
    );
    let entry_went =
        comes_to_hold(|| !kept.host.mob_entries_in_tests().contains(&kept.second_id)).await;
    assert!(entry_went, "the entry of the destroyed mob went");
    assert_eq!(
        open_below(&memory_of_the_second),
        Vec::<std::path::PathBuf>::new(),
        "the process holds no file of the memory of a mob whose entry went"
    );

    // The id is free: a create under it is admitted.
    let again = super::mob_entries_tests::second_mob(&kept.host, definition).await;
    assert_eq!(again, kept.second_id);
    shut_down(&kept.host, &[&kept.second_id]).await;
}
