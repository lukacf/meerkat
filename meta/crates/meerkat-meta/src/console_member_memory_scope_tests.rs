//! Where a member that is private to a person keeps its agent memory, and
//! who reads it through the Console.
//!
//! A Console with sign-in and access control for four people: Alice, Bob
//! and Carol, none an administrator, and root, the one administrator. Every
//! signed-in caller may do everything to every agent. The memory panel
//! reads the SQLite store, and a member's `memory` tool is the one its build
//! binds: the pre-build seam every member session create passes, given the
//! member's own roster labels and mob binding.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::SpawnMemberSpec;
use serde_json::{Value, json};

use super::console_private_side_chat_tests::{
    PRIVATE_TO, ROOT, TestResult, alice_claims, bearer, bob_claims, enforced_console,
    fork_on_console, root_claims,
};
use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
use super::console_token_lapse_tests::console_rpc_as;
use super::console_viewer_resolver_tests::CAROL;
use super::*;
use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryProvider, MEMORY_TOOL_NAME, MemoryRecorder,
    RecorderToolDispatcher,
};
use crate::memory::SqliteAgentMemoryStore;
use crate::memory::capabilities::MemoryPanelStore as _;
use crate::memory::capabilities::StewardStore as _;
use crate::memory::records::{MemoryRecord, MemoryScope, RecordStatus};
use crate::memory::side_chat_memory::SideChatMemorySlot;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// The shared member the side chats are forked from.
const SOURCE: &str = "alpha";
/// The member name that is used again.
const N: &str = "ledger-side";
/// Bob's own private side chat.
const BOBS_SIDE_CHAT: &str = "bob-side";
/// What the first owner's note holds.
const HERON: &str = "HERON-NOTE";
/// What the second owner's note holds.
const OTTER: &str = "OTTER-NOTE";
/// What the note of a member that a private member spawned holds.
const WREN: &str = "WREN-NOTE";

fn carol_claims() -> Value {
    json!({ "sub": "oidc-sub-carol", "email": CAROL, "provider": "google_oauth" })
}

/// Every signed-in caller may do everything to every agent and review
/// quarantined memory; root is the one administrator.
fn everyone_drives_and_reviews() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                id: "everyone-drives-every-agent".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec!["agent.*".to_string()],
                agents: vec!["*".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "everyone-reviews-quarantine".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec!["memory.quarantine.review".to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// The Console of `runtime`: sign-in for the four people, the access rules
/// on, the memory panel over `store`, and an identity runtime over the
/// runtime's own mob with agent memory in `store`.
async fn console_over(
    runtime: &MobRuntime,
    store: &Arc<SqliteAgentMemoryStore>,
    instance: &str,
) -> Result<Router, Failure> {
    use crate::identity_first::{
        AgentMemoryRuntimeInjector, DurabilityPolicy, IdentityRuntime, IdentityRuntimeConfig,
        LocalContinuityStore, LocalLeaseProvider, MobSessionBridge, PublicIdentityRuntime,
    };

    let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: instance.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(Arc::new(MobSessionBridge::new(runtime.handle()))),
        default_timeout: None,
    }));
    let provider: Arc<dyn AgentMemoryProvider> = store.clone();
    identity_runtime
        .set_agent_memory(Some(AgentMemoryRuntimeInjector::new(
            provider,
            AgentMemoryConfig::default(),
        )))
        .await;
    let mut state = enforced_console(runtime, Some(everyone_drives_and_reviews()));
    state.decisions.auth.email_allowlist.push(CAROL.to_string());
    let panel: Arc<dyn crate::memory::capabilities::MemoryPanelStore> = store.clone();
    state.memory_panel = Some(panel);
    state.identity_runtime = Some(PublicIdentityRuntime::new(
        identity_runtime,
        runtime.clone(),
    ));
    Ok(console_json_router_with_state(state))
}

/// The four people, as bearer tokens.
struct People {
    alice: String,
    bob: String,
    carol: String,
    root: String,
}

impl People {
    fn new() -> Self {
        Self {
            alice: bearer(alice_claims()),
            bob: bearer(bob_claims()),
            carol: bearer(carol_claims()),
            root: bearer(root_claims()),
        }
    }
}

/// One gateway with its memory store and its Console.
struct Stage {
    state: tempfile::TempDir,
    _memory_dir: tempfile::TempDir,
    store: Arc<SqliteAgentMemoryStore>,
    runtime: MobRuntime,
    /// The read of Core's identity entries of the runtime's mob, as the
    /// gateway's memory slot is bound to it.
    entries: Arc<dyn crate::identity_first::SessionBridge>,
    app: Router,
    people: People,
}

/// The session bridge over the mob of `runtime`.
fn identity_entries_of(runtime: &MobRuntime) -> Arc<dyn crate::identity_first::SessionBridge> {
    Arc::new(crate::identity_first::MobSessionBridge::new(
        runtime.handle(),
    ))
}

/// Labels as Core's build of a member puts them on the member's build
/// request: the roster labels, with the standard member labels of the
/// member's own binding stamped over them (`mob_id`, `role`, `profile_name`,
/// `meerkat_id` and `agent_identity`, the last two naming the member
/// itself).
fn as_built(
    mut labels: BTreeMap<String, String>,
    mob_id: &str,
    member: &str,
) -> BTreeMap<String, String> {
    for (key, value) in [
        ("mob_id", mob_id),
        ("role", "worker"),
        ("profile_name", "worker"),
        ("meerkat_id", member),
        ("agent_identity", member),
    ] {
        labels.insert(key.to_string(), value.to_string());
    }
    labels
}

impl Stage {
    async fn new(mob: &str) -> Result<Self, Failure> {
        let state = tempfile::tempdir()?;
        let memory_dir = tempfile::tempdir()?;
        let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
        seeded_member(&runtime, "worker", SOURCE, &[]).await?;
        let store = Arc::new(SqliteAgentMemoryStore::open(memory_dir.path())?);
        let app = console_over(&runtime, &store, mob).await?;
        Ok(Self {
            state,
            _memory_dir: memory_dir,
            store,
            entries: identity_entries_of(&runtime),
            runtime,
            app,
            people: People::new(),
        })
    }

    /// The caller scope the Console reports for the holder of `bearer`: the
    /// value a member private to that person carries as its owner.
    async fn owner_value(&self, bearer: &str) -> String {
        console_rpc_as(&self.app, Some(bearer), "meta/console/whoami", json!({})).await["result"]
            ["caller_scope"]
            .as_str()
            .expect("the caller scope")
            .to_string()
    }

    /// The holder of `bearer` opens a private side chat named `name`.
    async fn opens(&self, bearer: &str, name: &str) {
        let forked = fork_on_console(&self.app, Some(bearer), SOURCE, name).await;
        assert!(
            forked["error"].is_null(),
            "the private side chat {name} is opened: {forked:#}"
        );
    }

    /// The member `name` ends and leaves the roster.
    async fn ends(&self, name: &str) -> TestResult {
        let identity = AgentIdentity::from(name);
        for _ in 0..100 {
            match self.runtime.handle().retire(identity.clone()).await {
                Ok(()) => break,
                Err(error) => {
                    let text = error.to_string();
                    if !(text.contains("still in progress") || text.contains("work in flight")) {
                        return Err(error.into());
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for _ in 0..100 {
            if self.runtime.handle().get_member(&identity).await?.is_none() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(format!("{name} did not leave the roster").into())
    }

    /// The private side chat `side_chat` spawns a member named `name`: a
    /// spawn bound to the side chat's session, as the agent mob tools spawn
    /// one.
    async fn spawns(&self, side_chat: &str, name: &str) -> TestResult {
        self.spawns_member(side_chat, AgentIdentity::from(name))
            .await
    }

    /// [`Self::spawns`] for a member named by its roster id.
    async fn spawns_member(&self, side_chat: &str, member: AgentIdentity) -> TestResult {
        let identity = AgentIdentity::from(side_chat);
        self.runtime.handle().ensure_member_live(&identity).await?;
        let session = self
            .runtime
            .handle()
            .get_member(&identity)
            .await?
            .and_then(|entry| entry.bridge_session_id().cloned())
            .ok_or_else(|| format!("{side_chat} has a session"))?;
        Box::pin(
            self.runtime
                .handle()
                .spawn_spec_with_generated_owner_context(
                    SpawnMemberSpec::new("worker", member),
                    session,
                ),
        )
        .await?;
        Ok(())
    }

    /// Who the roster member `name` is private to.
    async fn owner_of(&self, name: &str) -> Result<Option<String>, Failure> {
        Ok(self
            .runtime
            .handle()
            .get_member(&AgentIdentity::from(name))
            .await?
            .ok_or_else(|| format!("{name} is seated"))?
            .labels
            .get(PRIVATE_TO)
            .cloned())
    }

    /// The `memory` tool of the roster member `name` as its build binds it,
    /// over an inherited overlay whose `memory` tool is bound to
    /// `inherited_from` (a fork inherits its source's).
    async fn memory_tool_of(
        &self,
        name: &str,
        inherited_from: &str,
    ) -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, Failure> {
        let identity = AgentIdentity::from(name);
        self.runtime.handle().ensure_member_live(&identity).await?;
        let entry = self
            .runtime
            .handle()
            .get_member(&identity)
            .await?
            .ok_or_else(|| format!("{name} is seated"))?;
        // The labels of the member's live session are the labels Core's
        // build of the member put on its build request, which the seam
        // reads: the roster labels with the standard member labels stamped
        // over them.
        let session = entry
            .bridge_session_id()
            .cloned()
            .ok_or_else(|| format!("{name} has a session"))?;
        let labels = self
            .runtime
            .session_service()
            .ok_or("the runtime has a session service")?
            .read(&session)
            .await?
            .state
            .labels;
        let mob_id = self.runtime.handle().mob_id().to_string();
        let stamped = as_built(entry.labels, &mob_id, name);
        for (key, value) in &stamped {
            assert_eq!(
                labels.get(key),
                Some(value),
                "the build request of {name} carries the label {key} of its roster labels and \
                 the standard member labels"
            );
        }
        Ok(memory_tool_for(
            &self.store,
            &self.entries,
            &mob_id,
            name,
            labels,
            inherited_from,
        ))
    }

    /// Every record the store holds, of every scope and status.
    async fn records(&self) -> Result<Vec<MemoryRecord>, Failure> {
        Ok(self
            .store
            .records_page(
                &AgentMemoryConfig::default().realm,
                None,
                None,
                None,
                100,
                None,
            )
            .await?
            .records)
    }

    /// The one stored record whose body holds `fact`.
    async fn record_holding(&self, fact: &str) -> Result<MemoryRecord, Failure> {
        let mut holding: Vec<MemoryRecord> = self
            .records()
            .await?
            .into_iter()
            .filter(|record| record.body.contains(fact))
            .collect();
        match holding.len() {
            1 => Ok(holding.remove(0)),
            count => Err(format!("{count} records hold {fact}").into()),
        }
    }

    /// What the holder of `bearer` reads of the record `memory_id` through
    /// the memory panel.
    async fn reads(&self, bearer: &str, name: &str, memory_id: &str, fact: &str) -> Reads {
        let holds = |answer: &Value| answer["error"].is_null() && answer.to_string().contains(fact);
        let by_name = console_rpc_as(
            &self.app,
            Some(bearer),
            "meta/memory/panel/records",
            json!({ "identity": name }),
        )
        .await;
        let listing = console_rpc_as(
            &self.app,
            Some(bearer),
            "meta/memory/panel/records",
            json!({}),
        )
        .await;
        let by_id = console_rpc_as(
            &self.app,
            Some(bearer),
            "meta/memory/panel/record",
            json!({ "memory_id": memory_id }),
        )
        .await;
        Reads {
            by_name: holds(&by_name),
            listing: holds(&listing),
            by_id: holds(&by_id),
        }
    }

    async fn stops(self) -> TestResult {
        self.runtime.handle().shutdown().await?;
        Ok(())
    }
}

/// Whether a caller was shown a note by the record listing of its member's
/// name, by the record listing that names no member, and by the record's
/// id.
#[derive(Debug, PartialEq, Eq)]
struct Reads {
    by_name: bool,
    listing: bool,
    by_id: bool,
}

impl Reads {
    const NOTHING: Self = Self {
        by_name: false,
        listing: false,
        by_id: false,
    };

    fn any(&self) -> bool {
        self.by_name || self.listing || self.by_id
    }
}

/// The `memory` tool of member `name` of mob `mob_id` as its build binds it
/// on a composition with an identity layer whose reader of Core's identity
/// entries is `entries`: `labels` are the labels of the member's build
/// request ([`as_built`]).
fn memory_tool_for(
    store: &Arc<SqliteAgentMemoryStore>,
    entries: &Arc<dyn crate::identity_first::SessionBridge>,
    mob_id: &str,
    name: &str,
    labels: BTreeMap<String, String>,
    inherited_from: &str,
) -> Arc<dyn meerkat_core::AgentToolDispatcher> {
    let provider: Arc<dyn AgentMemoryProvider> = store.clone();
    let config = AgentMemoryConfig::default();
    let slot = SideChatMemorySlot::default();
    slot.fill(Arc::clone(&provider), config.clone());
    slot.await_identity_entries();
    slot.watch_identity_entries(entries);
    let inherited: Arc<dyn meerkat_core::AgentToolDispatcher> =
        Arc::new(RecorderToolDispatcher::new(
            None,
            MemoryRecorder::new(
                Arc::clone(&provider),
                config,
                crate::identity_first::AgentIdentity::parse(inherited_from)
                    .expect("the source's identity"),
                Some(mob_id.to_string()),
            ),
        ));
    let mut req = meerkat_core::service::CreateSessionRequest {
        model: "gpt-5.5".to_string(),
        prompt: meerkat_core::types::ContentInput::Text("hello".to_string()),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        build: Some(meerkat_core::service::SessionBuildOptions {
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: mob_id.to_string(),
                role: "worker".to_string(),
                member: name.to_string(),
            }),
            external_tools: Some(inherited),
            ..Default::default()
        }),
        labels: Some(labels),
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
        injected_context: Vec::new(),
    };
    slot.bind_side_chat_memory(&mut req);
    req.build
        .and_then(|build| build.external_tools)
        .expect("the build keeps a memory tool")
}

/// One `memory` tool call: whether it failed, and its text.
async fn call_memory_tool(
    tool: &Arc<dyn meerkat_core::AgentToolDispatcher>,
    args: Value,
) -> (bool, String) {
    let args = serde_json::value::RawValue::from_string(args.to_string()).expect("tool arguments");
    match tool
        .dispatch(meerkat_core::types::ToolCallView {
            id: "call-1",
            name: MEMORY_TOOL_NAME,
            args: &args,
        })
        .await
    {
        Ok(outcome) => {
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
            (outcome.result.is_error, text)
        }
        Err(error) => (true, format!("dispatch failed: {error}")),
    }
}

fn remember(fact: &str) -> Value {
    json!({
        "action": "remember",
        "title": format!("{fact} title"),
        "body": format!("{fact}: a note of the member."),
    })
}

/// Alice opens the private side chat N, N's own `memory` tool stores her
/// note, and N ends. Returns the stored record.
async fn alices_member_stores_a_note_and_ends(stage: &Stage) -> Result<MemoryRecord, Failure> {
    stage.opens(&stage.people.alice, N).await;
    let tool = stage.memory_tool_of(N, SOURCE).await?;
    let (failed, text) = call_memory_tool(&tool, remember(HERON)).await;
    assert!(
        !failed && !text.contains("QUARANTINED"),
        "the first owner's member stores its note: {text}"
    );
    let record = stage.record_holding(HERON).await?;
    stage.ends(N).await?;
    Ok(record)
}

/// Bob opens his own private side chat, and it spawns a member named N,
/// which is private to Bob.
async fn bobs_member_takes_the_name(stage: &Stage) -> TestResult {
    stage.opens(&stage.people.bob, BOBS_SIDE_CHAT).await;
    stage.spawns(BOBS_SIDE_CHAT, N).await?;
    assert_eq!(
        stage.owner_of(N).await?,
        Some(stage.owner_value(&stage.people.bob).await),
        "the later member of the name is private to the second owner"
    );
    Ok(())
}

/// A member private to a second person, spawned under the name of an ended
/// member that was private to a first person, shows its owner none of the
/// first owner's records: not in the record listing of the name, not in the
/// listing that names no member, not by the record's id.
#[tokio::test]
async fn a_spawn_under_an_ended_private_members_name_shows_its_owner_none_of_the_earlier_records()
-> TestResult {
    let stage = Stage::new("memory-scope-spawn").await?;
    let record = alices_member_stores_a_note_and_ends(&stage).await?;
    bobs_member_takes_the_name(&stage).await?;

    assert_eq!(
        stage.reads(&stage.people.bob, N, &record.id, HERON).await,
        Reads::NOTHING,
        "the second owner reads none of the first owner's records"
    );
    assert_eq!(
        stage.reads(&stage.people.carol, N, &record.id, HERON).await,
        Reads::NOTHING,
        "a third person reads none of them"
    );
    stage.stops().await
}

/// A member private to a second person under the name of an ended member
/// of a first person, with the `memory` tool a fork inherits: it recalls
/// none of the first owner's records, changes none and removes none by id,
/// and the record stands as its member left it. A member of the first owner
/// seated under the name again afterwards recalls its own note and none of
/// the second owner's.
#[tokio::test]
async fn a_fork_under_an_ended_private_members_name_recalls_none_of_the_earlier_records()
-> TestResult {
    let stage = Stage::new("memory-scope-fork").await?;
    let record = alices_member_stores_a_note_and_ends(&stage).await?;
    bobs_member_takes_the_name(&stage).await?;

    let bobs = stage.memory_tool_of(N, BOBS_SIDE_CHAT).await?;
    for args in [
        json!({ "action": "recall" }),
        json!({ "action": "recall", "query_text": "Where is the note of the member?" }),
    ] {
        let (failed, text) = call_memory_tool(&bobs, args.clone()).await;
        assert!(
            !failed && !text.contains(HERON),
            "the later member recalls none of the earlier records ({args}): {text}"
        );
    }
    let (failed, text) = call_memory_tool(
        &bobs,
        json!({
            "action": "update",
            "memory_id": record.id,
            "title": "Changed",
            "body": "Changed by the later member.",
        }),
    )
    .await;
    assert!(failed, "the later member changes none of them: {text}");
    let (failed, text) =
        call_memory_tool(&bobs, json!({ "action": "forget", "memory_id": record.id })).await;
    assert!(failed, "the later member removes none of them: {text}");
    let (failed, text) = call_memory_tool(&bobs, remember(OTTER)).await;
    assert!(!failed, "the later member stores its own note: {text}");

    let kept = stage.record_holding(HERON).await?;
    assert_eq!(kept.status, RecordStatus::Active);
    assert_eq!(kept.body, record.body);
    assert_eq!(
        stage.records().await?.len(),
        2,
        "the store holds the two members' notes and no other record"
    );

    // The first owner's member is seated under the name again.
    stage.ends(N).await?;
    let alice = stage.owner_value(&stage.people.alice).await;
    seeded_member(&stage.runtime, "worker", N, &[(PRIVATE_TO, alice.as_str())]).await?;
    let alices = stage.memory_tool_of(N, N).await?;
    let (failed, text) = call_memory_tool(&alices, json!({ "action": "recall" })).await;
    assert!(
        !failed && text.contains(HERON) && !text.contains(OTTER),
        "the first owner's member recalls its own note and none of the other owner's: {text}"
    );
    stage.stops().await
}

/// While a member private to a second person holds the name, the first
/// owner still reads her ended member's records in the record listing and
/// by id, and by the key of their scope; the administrator reads them; the
/// second owner and a third person do not, by that key either.
#[tokio::test]
async fn the_first_owner_keeps_reading_her_records_while_another_owners_member_holds_the_name()
-> TestResult {
    let stage = Stage::new("memory-scope-owner-reads").await?;
    let record = alices_member_stores_a_note_and_ends(&stage).await?;
    bobs_member_takes_the_name(&stage).await?;

    let alice = stage.reads(&stage.people.alice, N, &record.id, HERON).await;
    assert!(
        alice.listing && alice.by_id,
        "the first owner reads her records in the listing and by id: {alice:?}"
    );
    let root = stage.reads(&stage.people.root, N, &record.id, HERON).await;
    assert!(
        root.listing && root.by_id,
        "the administrator reads them: {root:?}"
    );
    for (who, bearer) in [
        ("the second owner", &stage.people.bob),
        ("a third person", &stage.people.carol),
    ] {
        let reads = stage.reads(bearer, N, &record.id, HERON).await;
        assert!(!reads.any(), "{who} reads none of them: {reads:?}");
    }

    let key = format!(
        "private/{}/{N}",
        stage.owner_value(&stage.people.alice).await
    );
    assert_eq!(stage.record_holding(HERON).await?.scope.key(), key);
    for (who, bearer, shown) in [
        ("the first owner", &stage.people.alice, true),
        ("the administrator", &stage.people.root, true),
        ("the second owner", &stage.people.bob, false),
        ("a third person", &stage.people.carol, false),
    ] {
        let answer = console_rpc_as(
            &stage.app,
            Some(bearer),
            "meta/memory/panel/records",
            json!({ "scope": "identity", "scope_key": key }),
        )
        .await;
        assert_eq!(
            answer["error"].is_null() && answer.to_string().contains(HERON),
            shown,
            "{who} and the listing of the scope kept under the first owner: {answer:#}"
        );
    }
    stage.stops().await
}

/// An owner who opens a private member again under her ended private
/// member's name gets a member that recalls the earlier member's notes, and
/// reads them in the record listing of the name. They lie in the scope of
/// that owner and that name.
#[tokio::test]
async fn the_same_owner_recreating_its_member_finds_its_notes() -> TestResult {
    let stage = Stage::new("memory-scope-recreate").await?;
    let record = alices_member_stores_a_note_and_ends(&stage).await?;

    stage.opens(&stage.people.alice, N).await;
    let tool = stage.memory_tool_of(N, SOURCE).await?;
    let (failed, text) = call_memory_tool(&tool, json!({ "action": "recall" })).await;
    assert!(
        !failed && text.contains(HERON),
        "the owner's new member of the name recalls the earlier member's note: {text}"
    );
    let reads = stage.reads(&stage.people.alice, N, &record.id, HERON).await;
    assert_eq!(
        reads,
        Reads {
            by_name: true,
            listing: true,
            by_id: true,
        },
        "the owner reads her member's records"
    );
    assert_eq!(
        stage.record_holding(HERON).await?.scope,
        MemoryScope::Identity {
            realm: AgentMemoryConfig::default().realm,
            identity: format!(
                "private/{}/{N}",
                stage.owner_value(&stage.people.alice).await
            ),
        }
    );
    stage.stops().await
}

/// A scope kept under an owner is shown to the caller whose owner value
/// its key holds, compared as the label holds it: a caller whose value is
/// the key's escaped text, or differs from the owner in one escaped
/// character, or who owns nothing, is not shown it.
#[test]
fn an_owner_value_with_a_slash_a_percent_and_outer_spaces_reads_its_own_scope() {
    let access = everyone_drives_and_reviews();
    let key = "private/a%2Fb%25c/ledger-side";
    for (owner_value, shown) in [
        (Some("a/b%c"), true),
        (Some("a%2Fb%25c"), false),
        (Some("a/b%d"), false),
        (None, false),
    ] {
        let view = access
            .view_for_caller(Some("someone@example.test"), owner_value)
            .on_console(true);
        assert_eq!(
            memory_panel_identity_visible(MemoryPanelCaller::own(Some(&view)), key),
            shown,
            "a caller whose owner value is {owner_value:?} and the scope keyed '{key}'"
        );
    }
}

/// On a console that names its agents under a namespace, a scope's key is
/// read as the store holds it, never as the console spells a name: a
/// namespace called `private` makes no plain name a key and opens no scope
/// kept under another owner.
#[test]
fn a_namespace_spelling_is_never_read_as_a_scope_key() {
    let access = everyone_drives_and_reviews();
    let view = access
        .view_for_caller(Some("someone@example.test"), Some("a"))
        .on_console(true);
    let own = MemoryPanelCaller::own(Some(&view));
    let namespaced = MemoryPanelCaller {
        namespace: "private",
        ..own
    };
    assert_eq!(
        memory_panel_identity_visible(namespaced, "ledger-side"),
        memory_panel_identity_visible(own, "ledger-side"),
        "a plain name is judged as a plain name under the namespace"
    );
    assert!(
        memory_panel_identity_visible(namespaced, "private/a/ledger-side"),
        "the caller's own scope is shown under the namespace"
    );
    for key in [
        "private/b/ledger-side",
        "a/ledger-side",
        "private/ledger-side",
    ] {
        assert!(
            !memory_panel_identity_visible(namespaced, key),
            "the scope keyed '{key}' is not the caller's under the namespace"
        );
    }
}

/// The record of who a runtime's members are private to, unreadable: every
/// read of it fails.
struct UnreadableRecord;

#[async_trait::async_trait]
impl crate::private_owners::MemberSpawnLog for UnreadableRecord {
    async fn spawns_after(
        &self,
        _cursor: u64,
        _limit: usize,
    ) -> Result<Option<crate::private_owners::MemberSpawnPage>, String> {
        Err("the mob's event log cannot be read".to_string())
    }
}

/// After a restart, while the first read of who the members are private to
/// has failed, the owner of an ended private member reads its records, the
/// administrator reads them, and nobody else does. The same holds for a
/// record kept under her and the name of a member this gateway never knew
/// (a member of a mob that is gone).
#[tokio::test]
async fn the_owner_reads_an_ended_members_memory_before_the_owner_record_is_read_and_after_its_mob_is_gone()
-> TestResult {
    let mob = "memory-scope-restart";
    let stage = Stage::new(mob).await?;
    let record = alices_member_stores_a_note_and_ends(&stage).await?;
    let alice_value = stage.owner_value(&stage.people.alice).await;
    let gone = stage
        .store
        .remember_authored(
            &MemoryScope::Identity {
                realm: AgentMemoryConfig::default().realm,
                identity: format!("private/{alice_value}/gone-helper"),
            },
            crate::NewMemoryRecord {
                kind: crate::MemoryKind::Fact,
                title: format!("{OTTER} title"),
                description: String::new(),
                body: format!("{OTTER}: a note of a member of a mob that is gone."),
                tags: Vec::new(),
                evidence: Vec::new(),
                verification: None,
            },
            crate::memory::records::MemoryAuthor::Application,
        )
        .await?;
    let Stage {
        state,
        _memory_dir,
        store,
        runtime,
        app,
        people,
        ..
    } = stage;
    drop(app);
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(90))
        .await;

    let restarted = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    let runtime = MobRuntime::from_handle(restarted.handle());
    assert!(
        !runtime
            .private_owners()
            .seeded(&UnreadableRecord)
            .await
            .landed(),
        "the first read has not succeeded"
    );
    let app = console_over(&runtime, &store, mob).await?;
    let stage = Stage {
        state,
        _memory_dir,
        store,
        entries: identity_entries_of(&runtime),
        runtime,
        app,
        people,
    };

    for (memory_id, fact, name) in [
        (record.id.as_str(), HERON, N),
        (gone.memory_id.as_str(), OTTER, "gone-helper"),
    ] {
        let alice = stage
            .reads(&stage.people.alice, name, memory_id, fact)
            .await;
        assert!(
            alice.listing && alice.by_id,
            "the owner reads the records of {name}: {alice:?}"
        );
        let root = stage.reads(&stage.people.root, name, memory_id, fact).await;
        assert!(
            root.listing && root.by_id,
            "the administrator reads the records of {name}: {root:?}"
        );
        for (who, bearer) in [
            ("another person", &stage.people.bob),
            ("a third person", &stage.people.carol),
        ] {
            let reads = stage.reads(bearer, name, memory_id, fact).await;
            assert!(!reads.any(), "{who} reads no record of {name}: {reads:?}");
        }
    }
    restarted.handle().shutdown().await?;
    Ok(())
}

/// Two ended members of one name, each private to another person, in two
/// mobs of one memory realm, whichever mob's id sorts first: each owner
/// reads her own member's records and not the other's, a third person reads
/// neither, the administrator reads both.
#[tokio::test]
async fn with_both_holders_of_a_name_ended_in_two_mobs_each_owner_reads_her_own_and_no_other()
-> TestResult {
    let stage = Stage::new("memory-scope-two-mobs").await?;
    let alice = stage.owner_value(&stage.people.alice).await;
    let bob = stage.owner_value(&stage.people.bob).await;
    let private_to = |owner: &str| BTreeMap::from([(PRIVATE_TO.to_string(), owner.to_string())]);

    for (name, alices_mob, bobs_mob) in [
        ("ledger-one", "a-child-mob", "z-child-mob"),
        ("ledger-two", "z-child-mob", "a-child-mob"),
    ] {
        let herons = format!("{HERON}-{name}");
        let otters = format!("{OTTER}-{name}");
        for (mob, owner, fact) in [(alices_mob, &alice, &herons), (bobs_mob, &bob, &otters)] {
            let labels = private_to(owner);
            stage
                .runtime
                .private_owners()
                .record(mob, name, Some(&labels));
            let tool = memory_tool_for(
                &stage.store,
                &stage.entries,
                mob,
                name,
                as_built(labels, mob, name),
                name,
            );
            let (failed, text) = call_memory_tool(&tool, remember(fact)).await;
            assert!(!failed, "the member of {mob} stores its note: {text}");
        }
        let herons_id = stage.record_holding(&herons).await?.id;
        let otters_id = stage.record_holding(&otters).await?.id;

        for (who, bearer, own, others) in [
            (
                "the first owner",
                &stage.people.alice,
                (&herons_id, &herons),
                (&otters_id, &otters),
            ),
            (
                "the second owner",
                &stage.people.bob,
                (&otters_id, &otters),
                (&herons_id, &herons),
            ),
        ] {
            let reads = stage.reads(bearer, name, own.0, own.1).await;
            assert!(
                reads.listing && reads.by_id,
                "{who} reads her own member's records of {name}: {reads:?}"
            );
            let reads = stage.reads(bearer, name, others.0, others.1).await;
            assert!(
                !reads.any(),
                "{who} reads none of the other owner's records of {name}: {reads:?}"
            );
        }
        for (memory_id, fact) in [(&herons_id, &herons), (&otters_id, &otters)] {
            let reads = stage
                .reads(&stage.people.carol, name, memory_id, fact)
                .await;
            assert!(
                !reads.any(),
                "a third person reads no record of {name}: {reads:?}"
            );
            let reads = stage.reads(&stage.people.root, name, memory_id, fact).await;
            assert!(
                reads.listing && reads.by_id,
                "the administrator reads every record of {name}: {reads:?}"
            );
        }
    }
    stage.stops().await
}

/// A quarantined record of a private member is decided by its owner and by
/// an administrator, and by nobody else: not by a second person whose own
/// private member holds the name, who may decide for that member.
#[tokio::test]
async fn a_quarantined_record_under_an_owner_is_decided_by_its_owner_and_an_administrator_only()
-> TestResult {
    use crate::memory::TaintableStore as _;
    use crate::memory::records::MemoryAuthor;
    use crate::memory::staged::StagedBatchKind;

    struct HoldsEveryAgentWrite;
    impl crate::memory::taint::LlmWriteGate for HoldsEveryAgentWrite {
        fn quarantine_reason(
            &self,
            author: &MemoryAuthor,
            _kind: StagedBatchKind,
            _evidence: &[crate::memory::records::EvidenceRef],
        ) -> Option<String> {
            author
                .is_llm()
                .then(|| "the member read untrusted content".to_string())
        }
    }

    let stage = Stage::new("memory-scope-quarantine").await?;
    stage
        .store
        .set_llm_write_gate(Arc::new(HoldsEveryAgentWrite));
    stage.opens(&stage.people.alice, N).await;
    let tool = stage.memory_tool_of(N, SOURCE).await?;
    let mut held = Vec::new();
    for fact in [HERON, OTTER] {
        let (failed, text) = call_memory_tool(&tool, remember(fact)).await;
        assert!(
            !failed && text.contains("QUARANTINED"),
            "the member's write is stored quarantined: {text}"
        );
        let record = stage.record_holding(fact).await?;
        let hash = crate::memory::records::content_hash(&record.title, &record.body);
        held.push((record.id, hash));
    }
    stage.ends(N).await?;
    let decide = |record: &(String, String)| {
        json!({
            "identity": N,
            "memory_id": record.0,
            "verdict": "release",
            "expected_content_hash": record.1,
            "rationale": "checked with the owner",
        })
    };
    let quarantined =
        |record: MemoryRecord| matches!(record.status, RecordStatus::Quarantined { .. });

    // The name is free: its owner decides her member's first record.
    let decided = console_rpc_as(
        &stage.app,
        Some(&stage.people.alice),
        "meta/memory/quarantine/decide",
        decide(&held[0]),
    )
    .await;
    assert!(
        decided["error"].is_null() && decided["result"]["decision"].is_object(),
        "the owner decides her member's quarantined record: {decided:#}"
    );

    // A second person's private member holds the name.
    bobs_member_takes_the_name(&stage).await?;
    for (who, bearer) in [
        ("the second owner", &stage.people.bob),
        ("a third person", &stage.people.carol),
    ] {
        let decided = console_rpc_as(
            &stage.app,
            Some(bearer),
            "meta/memory/quarantine/decide",
            decide(&held[1]),
        )
        .await;
        assert!(
            !decided["error"].is_null(),
            "{who} decides no quarantined record of the first owner: {decided:#}"
        );
    }
    let records = stage
        .store
        .records_by_ids(
            &AgentMemoryConfig::default().realm,
            std::slice::from_ref(&held[1].0),
        )
        .await?;
    assert!(
        records.into_iter().all(quarantined),
        "the record is still quarantined"
    );
    let decided = console_rpc_as(
        &stage.app,
        Some(&stage.people.root),
        "meta/memory/quarantine/decide",
        decide(&held[1]),
    )
    .await;
    assert!(
        decided["error"].is_null() && decided["result"]["decision"].is_object(),
        "the administrator decides it: {decided:#}"
    );
    stage.stops().await
}

/// A member is refused, at its creation, a name that lies under a key
/// namespace agent memory reserves: a host's own spawn and a spawn a member
/// asks for alike. No member is seated under such a name.
#[tokio::test]
async fn a_member_name_under_a_reserved_memory_key_is_refused_at_creation() -> TestResult {
    let stage = Stage::new("memory-scope-reserved-name").await?;
    stage.opens(&stage.people.bob, BOBS_SIDE_CHAT).await;
    for name in [
        "private/console-caller:v1:aa/ledger-side",
        "set-aside/ledger-side",
    ] {
        let member = crate::member_comms_id::mob_member_id(name);
        let hosted = Box::pin(
            stage
                .runtime
                .handle()
                .spawn_spec(SpawnMemberSpec::new("worker", member.clone())),
        )
        .await;
        assert!(hosted.is_err(), "a host's spawn under '{name}' is refused");
        assert!(
            stage
                .spawns_member(BOBS_SIDE_CHAT, member.clone())
                .await
                .is_err(),
            "a member's spawn under '{name}' is refused"
        );
        assert!(
            stage.runtime.handle().get_member(&member).await?.is_none(),
            "no member is seated under '{name}'"
        );
    }
    stage.stops().await
}

/// No memory call that names an identity reaches a scope kept under an
/// owner: the shape is refused as an identity.
#[tokio::test]
async fn no_memory_call_names_a_scope_kept_under_an_owner() -> TestResult {
    let stage = Stage::new("memory-scope-no-name").await?;
    let record = alices_member_stores_a_note_and_ends(&stage).await?;
    let shaped = format!(
        "private/{}/{N}",
        stage.owner_value(&stage.people.alice).await
    );
    for (method, params) in [
        (
            "meta/agent_memory/recall",
            json!({ "identity": shaped, "selection": "always" }),
        ),
        (
            "meta/agent_memory/manifest",
            json!({ "identity": shaped, "tier": "full" }),
        ),
        (
            "meta/agent_memory/forget",
            json!({ "identity": shaped, "memory_id": record.id }),
        ),
        (
            "meta/memory/quarantine/decide",
            json!({
                "identity": shaped,
                "memory_id": record.id,
                "verdict": "tombstone",
                "expected_content_hash": "0",
                "rationale": "none",
            }),
        ),
    ] {
        let answer = console_rpc_as(&stage.app, Some(&stage.people.root), method, params).await;
        assert!(
            !answer["error"].is_null() && !answer.to_string().contains(HERON),
            "{method} refuses the shape as an identity: {answer:#}"
        );
    }
    assert_eq!(
        stage.record_holding(HERON).await?.status,
        RecordStatus::Active
    );
    stage.stops().await
}

/// What a member is told when it is to store the note that holds a fact.
fn store_ask(fact: &str) -> String {
    format!("Store the note {fact}.")
}

/// The id of the tool call that stores the note that holds a fact.
fn store_call_id(fact: &str) -> String {
    format!("call-store-{fact}")
}

/// A members' model that calls its member's own `memory` tool once for
/// each note its conversation asks it to store ([`store_ask`]), and answers
/// every other request in text.
#[derive(Clone, Default)]
struct NoteStoringModel {
    /// The messages of every request the model was sent, rendered.
    asked: Arc<std::sync::Mutex<Vec<String>>>,
}

impl NoteStoringModel {
    /// How many requests the model was sent, and the end of the last
    /// one's messages.
    fn last_asked(&self) -> String {
        let asked = self
            .asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let last = asked.last().map(String::as_str).unwrap_or_default();
        let end: String = last
            .chars()
            .skip(last.chars().count().saturating_sub(1500))
            .collect();
        format!("{} requests; the last one ends: {end}", asked.len())
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for NoteStoringModel {
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
        // The newest note the conversation asks for is stored once: the
        // request after the tool call carries the call, by its id, after
        // the ask, and is answered.
        let rendered = serde_json::to_string(&request.messages).unwrap_or_default();
        let fact = [HERON, OTTER, WREN]
            .into_iter()
            .filter_map(|fact| rendered.rfind(&store_ask(fact)).map(|at| (at, fact)))
            .max_by_key(|(at, _)| *at)
            .filter(|(at, fact)| !rendered[*at..].contains(&store_call_id(fact)))
            .map(|(_, fact)| fact);
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(rendered);
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            if fact.is_some() {
                meerkat_core::StopReason::ToolUse
            } else {
                meerkat_core::StopReason::EndTurn
            },
        );
        Box::pin(async_stream::stream! {
            match fact {
                Some(fact) => yield Ok(meerkat_client::LlmEvent::ToolCallComplete {
                    id: store_call_id(fact),
                    name: MEMORY_TOOL_NAME.to_string(),
                    args: remember(fact),
                    meta: None,
                }),
                None => yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
            }
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// The holder of `bearer` asks the member `to` through the Console's send
/// door to store the note that holds `fact`, and the note is stored: the
/// member's model called the member's own `memory` tool.
async fn sends_and_the_note_is_stored(
    app: &Router,
    bearer: &str,
    to: &str,
    (store, model): (&SqliteAgentMemoryStore, &NoteStoringModel),
    fact: &str,
) -> TestResult {
    let door = "meta/console/send";
    let sent = console_rpc_as(
        app,
        Some(bearer),
        door,
        json!({
            "identity": to,
            "content": store_ask(fact),
            "origin": "console",
            "idempotency_key": format!("store-{fact}"),
        }),
    )
    .await;
    assert!(
        sent["error"].is_null(),
        "{door} takes the message to {to}: {sent:#}"
    );
    let realm = AgentMemoryConfig::default().realm;
    for _ in 0..300 {
        let stored = store
            .records_page(&realm, None, None, None, 100, None)
            .await?
            .records
            .iter()
            .any(|record| record.body.contains(fact));
        if stored {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!(
        "no note that holds {fact} was stored after {door} to {to}; the members' model was sent {}",
        model.last_asked()
    )
    .into())
}

/// On a gateway's runtime composition that registers an agent (a state
/// directory, a roster, agent memory), a person opens a private side chat of
/// that agent through the Console's fork door and sends it a message
/// through the Console's send door, and the side chat's model calls the
/// side chat's own `memory` tool: the note is kept under the person and the
/// side chat's name, with the row that marks the scope, and under no plain
/// name. The registered agent's own note stays under the agent's name.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chat_of_a_registered_agent_keeps_what_its_own_memory_tool_stores_under_its_owner()
-> TestResult {
    a_private_side_chat_stores_a_note_with_its_own_memory_tool(true).await
}

/// The same on a runtime composition with no roster, whose source is a
/// member that is no registered agent's.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chat_on_a_launch_with_no_roster_keeps_what_its_own_memory_tool_stores_under_its_owner()
-> TestResult {
    a_private_side_chat_stores_a_note_with_its_own_memory_tool(false).await
}

/// The two rows above: the source `host-a` is a registered agent of the
/// launch's roster (`with_a_roster`), or a worker of a launch without one.
async fn a_private_side_chat_stores_a_note_with_its_own_memory_tool(
    with_a_roster: bool,
) -> TestResult {
    use crate::unified_runtime::held_identity_test_support::{Roster, member};

    const AGENT: &str = "host-a";
    let state = tempfile::tempdir()?;
    let memory_dir = tempfile::tempdir()?;
    let store = Arc::new(SqliteAgentMemoryStore::open(memory_dir.path())?);
    let config = AgentMemoryConfig::default();
    let realm = config.realm.clone();
    let model = NoteStoringModel::default();
    let mob_id = if with_a_roster {
        "memory-scope-with-a-roster"
    } else {
        "memory-scope-with-no-roster"
    };
    let builder = crate::UnifiedRuntime::builder()
        .definition(meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = '{mob_id}'\n[profiles.worker]\nmodel = 'gpt-5.5'\n\
             runtime_mode = 'turn_driven'\nexternal_addressable = true\n\
             [profiles.worker.tools]\ncomms = true\n"
        ))?)
        .persistent_state(state.path())
        .agent_memory(store.clone(), config)
        .default_llm_client(Arc::new(model.clone()));
    let builder = if with_a_roster {
        let agent = crate::identity_first::AgentIdentity::parse(AGENT)?;
        // A registered agent is given agent memory by its profile: the
        // builder's switch for the profile, which answers the same question
        // as the profile's own `tools.memory`.
        builder
            .continuity_from_state_dir(state.path())
            .await?
            .roster_provider(Arc::new(Roster(vec![member(&agent)])))
            .identity_runtime_instance_id(mob_id)
            .agent_memory_for_profile("worker", true)
    } else {
        builder
    };
    let runtime = Box::pin(builder.build()).await?;
    if !with_a_roster {
        runtime
            .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from(AGENT)))
            .await?;
    }
    let mut decisions = super::console_human_admission_tests::state().decisions;
    decisions
        .auth
        .email_allowlist
        .extend(["alice@example.test".to_string(), ROOT.to_string()]);
    let app = runtime.build_reference_app_router(decisions);
    let alice = bearer(alice_claims());
    let owner = console_rpc_as(&app, Some(&alice), "meta/console/whoami", json!({})).await["result"]
        ["caller_scope"]
        .as_str()
        .expect("the caller scope")
        .to_string();

    // The source's own note, and with it the build a fork of the source
    // inherits its per-spawn tools from.
    sends_and_the_note_is_stored(&app, &alice, AGENT, (&store, &model), OTTER).await?;
    if with_a_roster {
        assert!(
            matches!(
                runtime.mob_handle().identity_entry(
                    &crate::member_comms_id::roster_member_id_for_identity(AGENT)
                ),
                Ok(meerkat_mob::IdentityEntryRead::Entry(_))
            ),
            "the registered agent of the launch's roster is an identity entry of its mob"
        );
    }
    let forked = fork_on_console(&app, Some(&alice), AGENT, N).await;
    assert!(
        forked["error"].is_null(),
        "the private side chat is opened: {forked:#}"
    );
    assert_eq!(
        runtime
            .mob_handle()
            .get_member(&AgentIdentity::from(N))
            .await?
            .ok_or("the side chat is seated")?
            .labels
            .get(PRIVATE_TO),
        Some(&owner),
        "the side chat is private to the person who opened it"
    );
    sends_and_the_note_is_stored(&app, &alice, N, (&store, &model), HERON).await?;

    // A launch with no roster gives every member it spawns a `memory` tool
    // at its spawn: a member the private side chat spawns keeps its note
    // under the side chat's owner and its own name.
    const HELPER: &str = "ledger-side-helper";
    if !with_a_roster {
        let side_chat_session = runtime
            .mob_handle()
            .get_member(&AgentIdentity::from(N))
            .await?
            .and_then(|entry| entry.bridge_session_id().cloned())
            .ok_or("the side chat has a session")?;
        Box::pin(
            runtime
                .mob_handle()
                .spawn_spec_with_generated_owner_context(
                    SpawnMemberSpec::new("worker", AgentIdentity::from(HELPER)),
                    side_chat_session,
                ),
        )
        .await?;
        sends_and_the_note_is_stored(&app, &alice, HELPER, (&store, &model), WREN).await?;
    }

    let records = store
        .records_page(&realm, None, None, None, 100, None)
        .await?
        .records;
    let scope_of = |fact: &str| -> Vec<String> {
        records
            .iter()
            .filter(|record| record.body.contains(fact))
            .map(|record| record.scope.key().to_string())
            .collect()
    };
    let kept_under_owner = crate::memory::records::private_member_scope_key(&owner, N);
    assert_eq!(
        scope_of(HERON),
        vec![kept_under_owner.clone()],
        "the side chat's note is kept under its owner and its name"
    );
    assert!(
        store
            .private_retired_identities(&realm)
            .await?
            .contains(&kept_under_owner),
        "the scope is marked with its first record"
    );
    assert_eq!(
        scope_of(OTTER),
        vec![AGENT.to_string()],
        "the source's note is kept under the source's name"
    );
    if !with_a_roster {
        assert_eq!(
            scope_of(WREN),
            vec![crate::memory::records::private_member_scope_key(
                &owner, HELPER
            )],
            "the note of the member the side chat spawned is kept under the side chat's owner"
        );
    }
    let _ = Box::pin(runtime.shutdown()).await;
    Ok(())
}

/// A name under a key namespace agent memory reserves, as a roster member
/// id: the name a member an earlier release seated may hold.
fn kept_apart_member() -> AgentIdentity {
    crate::member_comms_id::mob_member_id("private/console-caller:v1:aa/ledger-side")
}

/// What a refusal of a member's name says of it.
const NAME_REFUSAL: &str = "lies under a name agent memory reserves";

/// What a spawn on the mob handle answered: `seated`, or its refusal.
fn spawn_answer<T>(outcome: Result<T, meerkat_mob::MobError>) -> String {
    match outcome {
        Ok(_) => "seated".to_string(),
        Err(error) => error.to_string(),
    }
}

/// The host's own durable fork of [`SOURCE`] on the mob handle into the
/// member `child`, retried while the source's last turn settles. Core does
/// not ask a host's customizer about the name of such a fork's child, so it
/// seats a member under a name the member creation doors refuse: what a
/// member seated by an earlier release looks like.
async fn host_forks(runtime: &MobRuntime, child: &AgentIdentity) -> TestResult {
    let mut refusal = String::new();
    for _ in 0..50 {
        match Box::pin(runtime.handle().fork_member(
            &AgentIdentity::from(SOURCE),
            SpawnMemberSpec::new("worker", child.clone()),
            None,
        ))
        .await
        {
            Ok(_) => return Ok(()),
            Err(error) => refusal = error.to_string(),
        }
        if !refusal.contains("running") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("the host's fork into {child} is not seated: {refusal}").into())
}

/// The runtime over `state` again after a shutdown of `runtime`.
async fn restarted(
    runtime: MobRuntime,
    state: &std::path::Path,
    mob: &str,
) -> Result<MobRuntime, Failure> {
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(90))
        .await;
    persistent_gateway_runtime_in(state, mob, "", "").await
}

/// The session the roster member `member` of `runtime` is bound to, with
/// the member live.
async fn live_session_of(
    runtime: &MobRuntime,
    member: &AgentIdentity,
) -> Result<meerkat_core::types::SessionId, Failure> {
    runtime.handle().ensure_member_live(member).await?;
    runtime
        .handle()
        .get_member(member)
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or_else(|| format!("{member} has a session").into())
}

/// The host retires the member `member` of `runtime`, and it leaves the
/// roster: its session stays, with its records of the member it ran as.
async fn host_retires(runtime: &MobRuntime, member: &AgentIdentity) -> TestResult {
    for _ in 0..100 {
        match runtime.handle().retire(member.clone()).await {
            Ok(()) => break,
            Err(error) => {
                let text = error.to_string();
                if !(text.contains("still in progress") || text.contains("work in flight")) {
                    return Err(error.into());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for _ in 0..100 {
        if runtime.handle().get_member(member).await?.is_none() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("{member} did not leave the roster").into())
}

/// A member's spawn with a resume launch creates a member on an existing
/// session. Under a name agent memory reserves it is refused for its name,
/// as the member's fresh spawn is: the spawn is bound to the asking
/// member's session, as Core's member tool `spawn_member` sends it, and
/// names a session nobody holds.
#[tokio::test]
async fn a_members_resume_spawn_under_a_reserved_memory_name_is_refused_for_its_name() -> TestResult
{
    let mob = "memory-scope-member-resume-spawn";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    seeded_member(&runtime, "worker", SOURCE, &[]).await?;
    let asking = live_session_of(&runtime, &AgentIdentity::from(SOURCE)).await?;
    let member = kept_apart_member();

    let answer = spawn_answer(
        Box::pin(
            runtime.handle().spawn_spec_with_generated_owner_context(
                SpawnMemberSpec::new("worker", member.clone())
                    .with_resume_bridge_session_id(meerkat_core::types::SessionId::new()),
                asking,
            ),
        )
        .await,
    );
    assert!(
        answer.contains(NAME_REFUSAL),
        "a member's resume spawn under the name is refused for the name: {answer}"
    );
    assert!(
        runtime.handle().get_member(&member).await?.is_none(),
        "no member is seated under the name"
    );
    runtime.handle().shutdown().await?;
    Ok(())
}

/// A host's own spawn on the mob handle with a resume launch is not asked
/// for its name: its request names no member that asked for it, as the
/// rebuild of a member already seated names none. Core admits a resume only
/// on a session whose own records name the same mob and the same member, so
/// such a spawn can only seat again a member that was seated under the name
/// before; on a session nobody holds Core refuses it, and not for the name.
#[tokio::test]
async fn a_hosts_own_resume_spawn_under_a_reserved_memory_name_is_not_refused_for_its_name()
-> TestResult {
    let mob = "memory-scope-host-resume-spawn";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    let member = kept_apart_member();

    let answer = spawn_answer(
        Box::pin(
            runtime.handle().spawn_spec(
                SpawnMemberSpec::new("worker", member.clone())
                    .with_resume_bridge_session_id(meerkat_core::types::SessionId::new()),
            ),
        )
        .await,
    );
    assert!(
        answer != "seated" && !answer.contains(NAME_REFUSAL),
        "a host's resume spawn on a session nobody holds is refused for the session, not for \
         the name: {answer}"
    );
    assert!(
        runtime.handle().get_member(&member).await?.is_none(),
        "no member is seated under the name"
    );
    runtime.handle().shutdown().await?;
    Ok(())
}

/// A fork-derived member seated under a name agent memory reserves is
/// rebuilt after a restart: the requests of its rebuild name no member that
/// asked for them and are not asked for the name.
#[tokio::test]
async fn a_fork_derived_member_under_a_reserved_memory_name_is_rebuilt_after_a_restart()
-> TestResult {
    let mob = "memory-scope-fork-rebuild";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    seeded_member(&runtime, "worker", SOURCE, &[]).await?;
    let member = kept_apart_member();
    host_forks(&runtime, &member).await?;
    let seated_on = live_session_of(&runtime, &member).await?;

    let runtime = restarted(runtime, state.path(), mob).await?;
    assert_eq!(
        live_session_of(&runtime, &member).await?,
        seated_on,
        "the member is rebuilt on its session after the restart"
    );
    runtime.handle().shutdown().await?;
    Ok(())
}

/// An ordinary member seated under a name agent memory reserves is rebuilt
/// after a restart. The member is seated here as only a host can seat one:
/// its own resume spawn on the mob handle, on the session a member of that
/// very name ran on before (a fork-derived member the host seated and
/// retired).
#[tokio::test]
async fn an_ordinary_member_under_a_reserved_memory_name_is_rebuilt_after_a_restart() -> TestResult
{
    let mob = "memory-scope-ordinary-rebuild";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    seeded_member(&runtime, "worker", SOURCE, &[]).await?;
    let member = kept_apart_member();
    host_forks(&runtime, &member).await?;
    let earlier = live_session_of(&runtime, &member).await?;
    host_retires(&runtime, &member).await?;

    let answer = spawn_answer(
        Box::pin(
            runtime.handle().spawn_spec(
                SpawnMemberSpec::new("worker", member.clone())
                    .with_resume_bridge_session_id(earlier.clone()),
            ),
        )
        .await,
    );
    assert_eq!(
        answer, "seated",
        "a host's resume spawn seats the name again on the session it ran on before"
    );
    let seated_on = live_session_of(&runtime, &member).await?;
    assert_eq!(seated_on, earlier, "on that very session");

    let runtime = restarted(runtime, state.path(), mob).await?;
    assert_eq!(
        live_session_of(&runtime, &member).await?,
        seated_on,
        "the member is rebuilt on its session after the restart"
    );
    runtime.handle().shutdown().await?;
    Ok(())
}

/// A member asks for a spawn with a resume launch onto the session a member
/// of a name agent memory reserves ran on before. Core admits a resume on
/// that session (its own records name the same mob and the same member), so
/// the refusal for the name is all that stands between the request and a
/// member seated under the name: nobody is seated. The earlier member is a
/// fork-derived member the host seated and retired; the spawn is bound to
/// the asking member's session, as Core's member tool `spawn_member` sends
/// it.
#[tokio::test]
async fn a_members_resume_spawn_onto_the_earlier_session_of_a_reserved_memory_name_seats_nobody()
-> TestResult {
    let mob = "memory-scope-member-resume-earlier";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    seeded_member(&runtime, "worker", SOURCE, &[]).await?;
    let member = kept_apart_member();
    host_forks(&runtime, &member).await?;
    let earlier = live_session_of(&runtime, &member).await?;
    host_retires(&runtime, &member).await?;
    let asking = live_session_of(&runtime, &AgentIdentity::from(SOURCE)).await?;

    let answer = spawn_answer(
        Box::pin(runtime.handle().spawn_spec_with_generated_owner_context(
            SpawnMemberSpec::new("worker", member.clone()).with_resume_bridge_session_id(earlier),
            asking,
        ))
        .await,
    );
    assert!(
        answer.contains(NAME_REFUSAL),
        "a member's resume spawn onto the name's earlier session is refused for the name: {answer}"
    );
    assert!(
        runtime.handle().get_member(&member).await?.is_none(),
        "no member is seated under the name"
    );
    runtime.handle().shutdown().await?;
    Ok(())
}
