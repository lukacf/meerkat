//! A host narrows a Meta-hosted member's mob operator authority, and the
//! member's own spawn tools admit against it, end to end through Meta's
//! production runtime.
//!
//! Meerkat gives a `tools.mob` member a default authority that may spawn
//! every profile of its mob. A host narrows it in its session hook
//! (`SessionHook::before_create`) with
//! `meerkat_runtime::mob_operator_authority::restrict_spawn_profiles_in_mob`,
//! the edit meerkat's own rows make (meerkat-mob
//! `runtime/tests/session_mob_authority.rs`). Meerkat binds the session's
//! final authority on the member's tools
//! (`AgentToolDispatcher::bind_session_mob_tool_authority`), and Meta's
//! wrappers of a member's tools forward that binding. Each row drives the
//! member's own turn: it calls `spawn_member` for a profile the hook removed,
//! `spawn_many_members` for a batch naming a kept and a removed profile, and
//! `spawn_member` for the kept profile, and reads each result as the model
//! receives it.
//!
//! The narrowing applies to one session build, and so does a disable of the
//! member's mob tools made in the hook: neither is restored as authority, and
//! a restore build reaches the host's hook with the profile's setting and
//! Meerkat's default authority again. A host edits every build, restores
//! included. The restart rows pin both sides of that contract.
//!
//! Storage: the host-spawned rows run on the builder's ephemeral composition
//! (in-memory mob storage, a temporary runtime store); the identity rows run
//! on `persistent_state` (durable mob storage and sessions), including a
//! restart.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::service::{CreateSessionRequest, SessionError, SessionQuery};
use meerkat_core::types::{StopReason, ToolCallView, ToolDef};
use meerkat_core::{AgentToolDispatcher, Message, ToolDispatchOutcome, ToolError};
use meerkat_meta::identity_first::contracts::{AgentCustomizer, RosterProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, CustomizerError,
    DurableAgentSpec, LocalExternalToolOverlay, RosterContext, RosterError,
};
use meerkat_meta::{DiscoverySpec, MetaConfig, SessionHook, UnifiedRuntime, UnifiedRuntimeBuilder};
use meerkat_mob::{MobDefinition, ProfileName, SpawnMemberSpec};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// Tests in this binary run concurrently, and one process-wide comms
/// namespace refuses two mobs with the same id.
static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

const PROBE: &str = "spawn-authority-probe";
const LEAD_PROFILE: &str = "lead";
const LEAD_IDENTITY: &str = "agent:lead";
/// The profile the host's hook keeps spawnable.
const KEPT_PROFILE: &str = "worker";
/// A profile of the same mob the hook removes.
const REMOVED_PROFILE: &str = "other";

/// `spawn_member` of the removed profile.
const SPAWN_REMOVED: &str = "o-1";
/// `spawn_many_members` of one kept and one removed profile.
const BATCH: &str = "batch-1";
const BATCH_KEPT: &str = "w-2";
const BATCH_REMOVED: &str = "o-2";
/// `spawn_member` of the kept profile.
const SPAWN_KEPT: &str = "w-1";

/// One call of the probe turn: its call id, tool and arguments.
#[derive(Clone, Debug)]
struct ProbeCall {
    id: &'static str,
    tool: &'static str,
    args: Value,
}

/// The probe turn's calls, in order: the removed profile alone, a batch that
/// names it beside the kept one, then the kept profile alone.
fn probe_calls() -> Vec<ProbeCall> {
    vec![
        ProbeCall {
            id: SPAWN_REMOVED,
            tool: "spawn_member",
            args: json!({ "profile": REMOVED_PROFILE, "member_id": SPAWN_REMOVED }),
        },
        ProbeCall {
            id: BATCH,
            tool: "spawn_many_members",
            args: json!({ "specs": [
                { "profile": KEPT_PROFILE, "member_id": BATCH_KEPT },
                { "profile": REMOVED_PROFILE, "member_id": BATCH_REMOVED },
            ] }),
        },
        ProbeCall {
            id: SPAWN_KEPT,
            tool: "spawn_member",
            args: json!({ "profile": KEPT_PROFILE, "member_id": SPAWN_KEPT }),
        },
    ]
}

/// Every profile of the mob: the spawn profiles of meerkat's default
/// authority for a `tools.mob` member.
fn every_profile() -> BTreeSet<String> {
    [LEAD_PROFILE, KEPT_PROFILE, REMOVED_PROFILE]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn unique_mob_id(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_MOB.fetch_add(1, Ordering::Relaxed))
}

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.{LEAD_PROFILE}]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.{LEAD_PROFILE}.tools]
comms = true
mob = true

[profiles.{KEPT_PROFILE}]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.{KEPT_PROFILE}.tools]
comms = true

[profiles.{REMOVED_PROFILE}]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.{REMOVED_PROFILE}.tools]
comms = true
"#
    ))
    .expect("parse the spawn authority definition")
}

/// The host's session hook. For each session build that carries mob operator
/// authority it records the spawn profiles of its mob the build arrived with,
/// and with `keep` it keeps only those profiles spawnable (without `keep` the
/// hook only observes). With `disable_mob_tools` it turns the member's mob
/// tools off instead. With `install_host_tools` the hook also installs the
/// host's own tools on that session, composed over the slot as hosts must
/// (`tool_compose::ComposedExternalTools`, as the RPC gateway does for
/// callback tools), so the member's own tools sit behind Meta's composition.
struct HostHook {
    mob_id: String,
    keep: Option<Vec<&'static str>>,
    install_host_tools: bool,
    disable_mob_tools: bool,
    /// Per build, in order: the spawn profiles of the hook's mob in the
    /// authority the build arrived with, before any narrowing.
    arrived_with: Mutex<Vec<BTreeSet<String>>>,
}

impl HostHook {
    fn new(mob_id: &str, keep: Option<&[&'static str]>, install_host_tools: bool) -> Arc<Self> {
        Arc::new(Self {
            mob_id: mob_id.to_string(),
            keep: keep.map(<[&'static str]>::to_vec),
            install_host_tools,
            disable_mob_tools: false,
            arrived_with: Mutex::new(Vec::new()),
        })
    }

    fn narrowing(mob_id: &str, keep: &[&'static str]) -> Arc<Self> {
        Self::new(mob_id, Some(keep), false)
    }

    fn narrowing_and_installing_tools(mob_id: &str, keep: &[&'static str]) -> Arc<Self> {
        Self::new(mob_id, Some(keep), true)
    }

    fn observing(mob_id: &str) -> Arc<Self> {
        Self::new(mob_id, None, false)
    }

    /// Turns the member's mob tools off: no mob operator tools, no
    /// authority.
    fn disabling(mob_id: &str) -> Arc<Self> {
        Arc::new(Self {
            mob_id: mob_id.to_string(),
            keep: None,
            install_host_tools: false,
            disable_mob_tools: true,
            arrived_with: Mutex::new(Vec::new()),
        })
    }

    /// How many session builds carrying mob operator authority this hook saw
    /// (and, with `keep` or `disable_mob_tools`, edited).
    fn sessions_with_authority(&self) -> usize {
        self.arrived_with.lock().unwrap().len()
    }

    /// The spawn profiles each of those builds arrived with, in order.
    fn arrived_with(&self) -> Vec<BTreeSet<String>> {
        self.arrived_with.lock().unwrap().clone()
    }
}

#[async_trait]
impl SessionHook for HostHook {
    async fn before_create(&self, req: &mut CreateSessionRequest) -> Result<(), SessionError> {
        let Some(authority) = req
            .build
            .as_ref()
            .and_then(|build| build.mob_tool_authority_context.as_ref())
        else {
            return Ok(());
        };
        self.arrived_with.lock().unwrap().push(
            authority
                .spawn_profile_scope()
                .get(&self.mob_id)
                .cloned()
                .unwrap_or_default(),
        );
        if self.disable_mob_tools {
            if let Some(build) = req.build.as_mut() {
                build.override_mob = meerkat_core::ToolCategoryOverride::Disable;
                build.mob_tool_authority_context = None;
            }
            return Ok(());
        }
        let Some(keep) = &self.keep else {
            return Ok(());
        };
        let narrowed = meerkat_runtime::mob_operator_authority::restrict_spawn_profiles_in_mob(
            authority,
            &self.mob_id,
            keep.iter().copied(),
        )
        .map_err(SessionError::Unsupported)?;
        if let Some(build) = req.build.as_mut() {
            build.mob_tool_authority_context = Some(narrowed);
            if self.install_host_tools {
                let pre_installed = build.external_tools.take();
                build.external_tools =
                    Some(meerkat_meta::tool_compose::ComposedExternalTools::over(
                        Arc::new(HostTools),
                        pre_installed,
                    ));
            }
        }
        Ok(())
    }
}

/// The host's own tool: installed by [`HostHook`] or published by the
/// identity's customizer.
struct HostTools;

#[async_trait]
impl AgentToolDispatcher for HostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef {
            audience: Default::default(),
            name: "lookup_workspace".into(),
            description: "Look up the workspace record.".to_string(),
            input_schema: json!({"type": "object"}),
            provenance: None,
        })]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(meerkat_core::ToolResult {
            host_metadata: Default::default(),
            tool_use_id: call.id.to_string(),
            content: vec![],
            is_error: false,
            settlement_failures: Vec::new(),
        }
        .into())
    }
}

/// Makes [`probe_calls`] on the probe turn, then ends the turn and records
/// each call's tool-result text by call id, with the tools the member had
/// mounted.
#[derive(Clone, Default)]
struct ProbeClient {
    results: Arc<Mutex<Option<BTreeMap<String, String>>>>,
    mounted: Arc<Mutex<Vec<String>>>,
    done: Arc<tokio::sync::Notify>,
}

impl ProbeClient {
    fn take_results(&self) -> BTreeMap<String, String> {
        self.results
            .lock()
            .unwrap()
            .take()
            .expect("the probe turn recorded its results")
    }

    fn mounted(&self) -> Vec<String> {
        self.mounted.lock().unwrap().clone()
    }
}

/// Tool-result text per call id in `messages`.
fn tool_results(messages: &[Message]) -> BTreeMap<String, String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.tool_use_id.clone(), result.text_content()))
        .collect()
}

fn is_probe_turn(request: &LlmRequest) -> bool {
    // The probe text reaches the member wrapped as its runtime delivers it;
    // look for it anywhere in the transcript.
    serde_json::to_string(&request.messages).is_ok_and(|messages| messages.contains(PROBE))
}

impl LlmClient for ProbeClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let provider = LlmClient::provider(self);
        let next = if is_probe_turn(request) {
            *self.mounted.lock().unwrap() = request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            let results = tool_results(&request.messages);
            let next = probe_calls()
                .into_iter()
                .find(|call| !results.contains_key(call.id));
            if next.is_none() {
                *self.results.lock().unwrap() = Some(results);
                self.done.notify_waiters();
            }
            next
        } else {
            None
        };
        let stop = if next.is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        let [usage, done] = llm_usage::usage_then_done(request, provider, stop);
        Box::pin(async_stream::stream! {
            match next {
                Some(call) => {
                    yield Ok(LlmEvent::ToolCallComplete {
                        id: call.id.to_string(),
                        name: call.tool.to_string(),
                        args: call.args,
                        meta: None,
                    });
                }
                None => {
                    yield Ok(LlmEvent::TextDelta { delta: "probed".to_string(), meta: None });
                }
            }
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

fn module_config(mob_id: &str) -> MetaConfig {
    MetaConfig {
        modules: Vec::new(),
        discovery: DiscoverySpec {
            namespace: mob_id.to_string(),
            modules: Vec::new(),
        },
        pre_spawn: Vec::new(),
    }
}

/// A runtime on the builder's ephemeral composition whose lead the host
/// spawns directly (`spawn_many`), with the host's session hook when given.
async fn host_spawned_runtime(
    mob_id: &str,
    client: &ProbeClient,
    hook: Option<Arc<HostHook>>,
) -> UnifiedRuntime {
    let mut builder = UnifiedRuntime::builder()
        .definition(definition(mob_id))
        .module_config(module_config(mob_id))
        .timeout(Duration::from_secs(30))
        .default_llm_client(Arc::new(client.clone()));
    if let Some(hook) = hook {
        builder = builder.session_hook(hook);
    }
    let runtime = Box::pin(builder.build()).await.expect("runtime builds");
    runtime
        .spawn_many(vec![SpawnMemberSpec::new(LEAD_PROFILE, "lead-1")])
        .await
        .expect("the lead builds");
    runtime
}

/// Sends the probe to the host-spawned lead and returns each call's result.
async fn probe_host_spawned_lead(
    runtime: &UnifiedRuntime,
    client: &ProbeClient,
) -> BTreeMap<String, String> {
    let done = client.done.notified();
    tokio::pin!(done);
    done.as_mut().enable();
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "lead-1", PROBE.to_string())
        .await
        .expect("send the probe turn");
    tokio::time::timeout(Duration::from_mins(1), done)
        .await
        .expect("the probe turn reaches its final request");
    client.take_results()
}

fn result_text<'a>(results: &'a BTreeMap<String, String>, call: &str) -> &'a str {
    results
        .get(call)
        .unwrap_or_else(|| panic!("call {call} was made: {results:?}"))
}

/// The spawn admission's refusal (`ToolError::access_denied` for the tool),
/// not some other failure.
fn assert_refused(results: &BTreeMap<String, String>, call: &str, tool: &str) {
    let text = result_text(results, call);
    assert!(
        text.contains("access_denied") && text.contains(tool),
        "{call}: {tool} is refused by the member's spawn authority: {text}"
    );
}

fn assert_spawned(results: &BTreeMap<String, String>, call: &str, member: &str) {
    let text = result_text(results, call);
    let payload: Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("{call}: spawning {member} succeeds: {text} ({error})"));
    assert_eq!(payload["agent_identity"], member, "{payload}");
}

/// The batch is admitted only whole: every spawned entry named.
fn assert_batch_spawned(results: &BTreeMap<String, String>, members: &[&str]) {
    let text = result_text(results, BATCH);
    for member in members {
        assert!(text.contains(member), "the batch spawns {member}: {text}");
    }
}

/// The mob's members and the runtime's sessions after the probe: each of
/// `present` is a member and none of `absent` is, and the runtime holds
/// exactly the lead's session and one per spawned member, so a refused spawn
/// built no session either (member sessions carry no labels to look up).
async fn assert_members(runtime: &UnifiedRuntime, present: &[&str], absent: &[&str]) {
    let members: Vec<String> = runtime
        .mob_handle()
        .list_members()
        .await
        .into_iter()
        .map(|member| member.agent_identity.to_string())
        .collect();
    for member in present {
        assert!(
            members.iter().any(|name| name == member),
            "{member} is a member: {members:?}"
        );
    }
    for member in absent {
        assert!(
            !members.iter().any(|name| name == member),
            "{member} was never spawned: {members:?}"
        );
    }
    let sessions = runtime
        .mob_runtime()
        .session_service()
        .expect("the runtime's session service")
        .list(SessionQuery::default())
        .await
        .expect("list the runtime's sessions");
    assert_eq!(
        sessions.len(),
        1 + present.len(),
        "the lead's session and one per spawned member: {sessions:?}"
    );
}

/// The narrowed outcome every hooked row asserts: the removed profile is
/// refused alone and in a batch (the batch is admitted whole or not at all,
/// so its kept entry is not spawned either), the kept profile is spawned, and
/// `spawn_member` stays listed: the narrowing restricts spawn profiles, not
/// the tools the member sees.
async fn assert_narrowed_spawns(
    runtime: &UnifiedRuntime,
    results: &BTreeMap<String, String>,
    mounted: &[String],
) {
    for tool in ["spawn_member", "spawn_many_members"] {
        assert!(
            mounted.iter().any(|name| name == tool),
            "the narrowed lead still lists {tool}: {mounted:?}"
        );
    }
    assert_refused(results, SPAWN_REMOVED, "spawn_member");
    assert_refused(results, BATCH, "spawn_many_members");
    assert_spawned(results, SPAWN_KEPT, SPAWN_KEPT);
    assert_members(
        runtime,
        &[SPAWN_KEPT],
        &[SPAWN_REMOVED, BATCH_KEPT, BATCH_REMOVED],
    )
    .await;
}

/// The default outcome: meerkat's default authority for a `tools.mob` member
/// spawns every profile of its mob, alone and in a batch.
async fn assert_default_spawns(runtime: &UnifiedRuntime, results: &BTreeMap<String, String>) {
    assert_spawned(results, SPAWN_REMOVED, SPAWN_REMOVED);
    assert_batch_spawned(results, &[BATCH_KEPT, BATCH_REMOVED]);
    assert_spawned(results, SPAWN_KEPT, SPAWN_KEPT);
    assert_members(
        runtime,
        &[SPAWN_REMOVED, BATCH_KEPT, BATCH_REMOVED, SPAWN_KEPT],
        &[],
    )
    .await;
}

/// A member the host spawns, whose session the host's hook narrowed to the
/// kept profile, spawns that profile and is refused the removed one.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_narrowed_member_spawns_only_the_profiles_its_hook_grants() {
    let mob_id = unique_mob_id("narrowed-spawn");
    let hook = HostHook::narrowing(&mob_id, &[KEPT_PROFILE]);
    let client = ProbeClient::default();
    let runtime = host_spawned_runtime(&mob_id, &client, Some(Arc::clone(&hook))).await;
    assert_eq!(
        hook.sessions_with_authority(),
        1,
        "the hook narrowed the lead's session, and no other"
    );

    let results = probe_host_spawned_lead(&runtime, &client).await;
    assert_narrowed_spawns(&runtime, &results, &client.mounted()).await;
    let report = runtime.shutdown().await;
    assert!(report.mob_stop.is_ok(), "{:?}", report.mob_stop);
}

/// The host's hook also installs its own tools on the lead's session,
/// composed over the slot (`ComposedExternalTools`, as the RPC gateway does
/// for callback tools). The member's own tools then sit behind Meta's
/// composition, which must forward meerkat's binding of the narrowed
/// authority to them.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_narrowed_member_with_host_composed_tools_spawns_only_the_granted_profiles() {
    let mob_id = unique_mob_id("composed-narrowed-spawn");
    let hook = HostHook::narrowing_and_installing_tools(&mob_id, &[KEPT_PROFILE]);
    let client = ProbeClient::default();
    let runtime = host_spawned_runtime(&mob_id, &client, Some(Arc::clone(&hook))).await;
    assert_eq!(
        hook.sessions_with_authority(),
        1,
        "the hook narrowed the lead's session, and no other"
    );

    let results = probe_host_spawned_lead(&runtime, &client).await;
    let mounted = client.mounted();
    assert!(
        mounted.iter().any(|name| name == "lookup_workspace"),
        "the lead mounts the host's tool over its own: {mounted:?}"
    );
    assert_narrowed_spawns(&runtime, &results, &mounted).await;
    let report = runtime.shutdown().await;
    assert!(report.mob_stop.is_ok(), "{:?}", report.mob_stop);
}

/// Without a hook the lead keeps meerkat's default authority, which may spawn
/// every profile of its mob: every spawn is admitted, so the narrowing is
/// what refuses the removed profile in the rows above.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_hook_the_member_spawns_every_profile_of_its_mob() {
    let mob_id = unique_mob_id("default-spawn");
    let client = ProbeClient::default();
    let runtime = host_spawned_runtime(&mob_id, &client, None).await;

    let results = probe_host_spawned_lead(&runtime, &client).await;
    assert_default_spawns(&runtime, &results).await;
    let report = runtime.shutdown().await;
    assert!(report.mob_stop.is_ok(), "{:?}", report.mob_stop);
}

struct HostToolCustomizer;

#[async_trait]
impl AgentCustomizer for HostToolCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(HostTools));
        Ok(())
    }
}

struct LeadRoster;

#[async_trait]
impl RosterProvider for LeadRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![DurableAgentSpec {
            identity: AgentIdentity::parse(LEAD_IDENTITY).expect("lead identity"),
            profile: ProfileName::from(LEAD_PROFILE),
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
        }])
    }
}

/// An identity-first runtime on `persistent_state` whose lead identity has
/// host customizer tools, with the host's narrowing hook.
async fn identity_runtime(
    state: &std::path::Path,
    mob_id: &str,
    client: &ProbeClient,
    hook: Arc<HostHook>,
) -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition(mob_id))
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(LeadRoster))
            .agent_customizer(Arc::new(HostToolCustomizer))
            .session_hook(hook)
            .identity_runtime_instance_id(mob_id)
            .module_config(module_config(mob_id))
            .timeout(Duration::from_secs(30))
            .default_llm_client(Arc::new(client.clone()))
            .build(),
    )
    .await
    .expect("build the identity-first runtime")
}

/// Sends the probe to the lead identity and returns each call's result.
async fn probe_lead_identity(
    runtime: &UnifiedRuntime,
    client: &ProbeClient,
) -> BTreeMap<String, String> {
    let identities = runtime.identity_runtime().expect("identity runtime");
    identities
        .send_awaiting_commit(
            &AgentIdentity::parse(LEAD_IDENTITY).expect("lead identity"),
            &meerkat_core::ContentInput::Text(PROBE.to_string()),
        )
        .await
        .expect("the probe turn commits");
    client.take_results()
}

async fn lead_session(runtime: &UnifiedRuntime) -> String {
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .status(&AgentIdentity::parse(LEAD_IDENTITY).expect("lead identity"))
        .await
        .expect("lead status")
        .session_id
        .expect("the lead has a session")
        .to_string()
}

/// An identity-first lead on `persistent_state` whose host customizer
/// publishes tools for it: the session the host's hook narrowed spawns only
/// the kept profile.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_member_with_customizer_tools_spawns_only_the_profiles_its_hook_grants() {
    let state = tempfile::tempdir().expect("state dir");
    let mob_id = unique_mob_id("identity-narrowed-spawn");
    let hook = HostHook::narrowing(&mob_id, &[KEPT_PROFILE]);
    let client = ProbeClient::default();
    let runtime = identity_runtime(state.path(), &mob_id, &client, Arc::clone(&hook)).await;
    assert!(
        runtime
            .identity_runtime()
            .expect("identity runtime")
            .identity_bootstrap_status()
            .ready
    );
    assert_eq!(
        hook.sessions_with_authority(),
        1,
        "the hook narrowed the lead's session, and no other"
    );

    let results = probe_lead_identity(&runtime, &client).await;
    let mounted = client.mounted();
    assert!(
        mounted.iter().any(|name| name == "lookup_workspace"),
        "the lead mounts its customizer tool: {mounted:?}"
    );
    assert_narrowed_spawns(&runtime, &results, &mounted).await;
    let shutdown = runtime.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
}

/// Boots the identity-first lead with `first_hook`, which edits its build,
/// stops it, and boots it again on the same state with `second_hook`; returns
/// the restored runtime once its lead holds the same session. Both boots'
/// builds of the lead reach their hook with Meerkat's default authority: the
/// restore build is built through the hook again rather than restoring what
/// the first boot's hook made of it.
async fn restart_lead(
    state: &std::path::Path,
    mob_id: &str,
    client: &ProbeClient,
    first_hook: Arc<HostHook>,
    second_hook: Arc<HostHook>,
) -> UnifiedRuntime {
    let first = identity_runtime(state, mob_id, client, Arc::clone(&first_hook)).await;
    assert_eq!(
        first_hook.arrived_with(),
        vec![every_profile()],
        "the first boot's hook edited the lead's build, which arrived with the default authority"
    );
    let session = lead_session(&first).await;
    let shutdown = first.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
    drop(first);

    let second = identity_runtime(state, mob_id, client, Arc::clone(&second_hook)).await;
    assert_eq!(
        lead_session(&second).await,
        session,
        "the second boot restores the lead's session"
    );
    assert_eq!(
        second_hook.sessions_with_authority(),
        1,
        "the lead's restore build reaches the second boot's hook, and no other build carries authority"
    );
    assert_eq!(
        second_hook.arrived_with(),
        vec![every_profile()],
        "the restore build arrives with the default authority, not what the first boot's hook made of it"
    );
    second
}

/// After a restart the restored lead (the same session, rebuilt as the second
/// boot restores it) is still refused the removed profile: the host's hook,
/// installed on both boots, narrows the restore build as it narrowed the
/// first one.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_identity_member_stays_narrowed_after_a_restart() {
    let state = tempfile::tempdir().expect("state dir");
    let mob_id = unique_mob_id("restored-narrowed-spawn");
    let client = ProbeClient::default();
    let second = restart_lead(
        state.path(),
        &mob_id,
        &client,
        HostHook::narrowing(&mob_id, &[KEPT_PROFILE]),
        HostHook::narrowing(&mob_id, &[KEPT_PROFILE]),
    )
    .await;

    let results = probe_lead_identity(&second, &client).await;
    assert_narrowed_spawns(&second, &results, &client.mounted()).await;
    let shutdown = second.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
}

/// The contract witness: a host's narrowing applies per session build, so a
/// restored member's authority follows the hook of the boot that restores it.
/// The restore build reaches that hook with the default authority
/// (`restart_lead`); a hook that does not narrow it leaves the
/// restored lead (the same session) with the default, which spawns every
/// profile of its mob. A host narrows every build, restores included, as in
/// the row above.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_member_follows_the_current_host_hook_after_a_restart() {
    let state = tempfile::tempdir().expect("state dir");
    let mob_id = unique_mob_id("restored-unnarrowed-spawn");
    let client = ProbeClient::default();
    let second = restart_lead(
        state.path(),
        &mob_id,
        &client,
        HostHook::narrowing(&mob_id, &[KEPT_PROFILE]),
        HostHook::observing(&mob_id),
    )
    .await;

    let results = probe_lead_identity(&second, &client).await;
    assert_default_spawns(&second, &results).await;
    let shutdown = second.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
}

/// A disable of the member's mob tools that the hook made on an earlier build
/// is per build too. The first boot's hook turns the lead's mob tools off;
/// the restore build reaches the second boot's hook with the profile's
/// setting and the default authority again (`restart_lead`), and a hook that
/// leaves it alone gives the restored lead (the same session) its mob tools
/// back, with every profile of its mob spawnable.
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_tools_disable_made_by_the_hook_is_per_build_too() {
    let state = tempfile::tempdir().expect("state dir");
    let mob_id = unique_mob_id("restored-undisabled-spawn");
    let client = ProbeClient::default();
    let second = restart_lead(
        state.path(),
        &mob_id,
        &client,
        HostHook::disabling(&mob_id),
        HostHook::observing(&mob_id),
    )
    .await;

    let results = probe_lead_identity(&second, &client).await;
    let mounted = client.mounted();
    for tool in ["spawn_member", "spawn_many_members"] {
        assert!(
            mounted.iter().any(|name| name == tool),
            "the restored lead lists {tool} again: {mounted:?}"
        );
    }
    assert_default_spawns(&second, &results).await;
    let shutdown = second.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
}
