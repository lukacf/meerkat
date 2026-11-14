//! `customize_build` tools across a restart (#563), on the gateway-shaped
//! harness of `identity_first_head_canonical_resume` (state-dir continuity
//! substrate, persistent state, recording LLM double). Core holds every
//! roster identity's member and builds it only when it materializes it: on
//! its first delivery, or at a start that restores it (a member that owes
//! work, or every member under `MemberRestore::Eager`). Before each build it
//! asks Meta's member build preparer.
//!
//! - Nothing is published before the mob is built or lifted, and no pass
//!   seeds tool records: a clean restart over a Dormant member builds nothing
//!   and calls the host's `customize_build` for no member.
//!   `meta/identity/resolved_tools` answers a Dormant member's last build
//!   from its tool record, on its durable session, and the read wakes
//!   nothing. An identity never built answers the typed `no_build_recorded`
//!   with its session until its first delivery builds it.
//! - A wake once the identity runtime is composed prepares the member's
//!   build inline: one `customize_build`, run with the runtime's real context
//!   (the mob handle, the registered roster), publishes its tools before
//!   Core builds the member, so its first turn offers them. A refused
//!   preparation keeps the member Dormant and refuses the delivery
//!   retryable with the host's reason; the next delivery tries again.
//! - A governed input after a clean restart wakes its member the same way:
//!   one `customize_build`, with the inputs of the member's last build before
//!   the restart, answered before the host's bind, and no `after_create`; the
//!   member's next turn calls the tool through the handler that build
//!   registered. A runtime the builder composes pins the member's controller,
//!   so governed input reaches the host's bind through both doors.
//! - A member Core builds before the composition (a start that restores
//!   every member, or one whose run a kill left open) is held
//!   (`HostRunStartHoldReason::ToolsNotPublished`) until the roster pass
//!   publishes its tools and releases the hold: no run starts without them.
//!   A publication that fails keeps the member held, and work Core admits
//!   for it waits until a delivery publishes them.
//! - A host that composes with `UnifiedRuntime::bootstrap` gets the same
//!   hold across a kill when its spec declares the agent customizer
//!   (`MobBootstrapSpec::with_agent_customizer`).
//! - A restored member whose identity the roster no longer lists starts no
//!   run before the roster pass retires it. A host-adopted identity's member,
//!   which the roster pass never retires and no roster publication covers,
//!   is not held: it runs as before.
//! - A host-adopted member that a process restart revives after a crash (the
//!   first process ended without shutdown, holding its identity lease) is
//!   converged again promptly, and work submitted to it runs.
//! - A durable member spawned at runtime, which no roster lists, is neither
//!   held nor retired by the roster pass after a kill or a graceful
//!   shutdown: its open run continues from its committed round, while an
//!   identity the roster really dropped is still retired. The resumed runs
//!   reach the host's event log through Meta's forwarded member events,
//!   from their resume to their terminal, once.
//! - A member restored without its comms identity reads broken everywhere.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{StopReason, ToolCallView, ToolDef};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, RosterProvider, TopologyProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, BoundGovernedInput,
    CustomizerError, DurableAgentSpec, GovernedBindContext, GovernedBindError, GovernedBindRefusal,
    GovernedInputBinder, GovernedInputError, GovernedSettlement, IdentityBootstrapMode,
    IdentityLifecycleState, IdentityRuntimeError, IdentityStatus, LocalExternalToolOverlay,
    ManagedPeerEdge, MemberRestore, RosterContext, RosterError, TopologyContext, TopologyError,
};
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;
use meerkat_mob::{MobDefinition, ProfileName};
use tokio::time::sleep;

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// `{MOB_ID}` is replaced per test: tests in this binary run concurrently and
/// one process-wide comms namespace refuses two mobs with the same id.
const MOB_TOML: &str = r#"
[mob]
id = "{MOB_ID}"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

const MEMBER: &str = "personal:alice";
const TOOL: &str = "lookup_workspace";
/// Work Core admits for a held member.
const QUEUED_WORK: &str = "work queued behind the hold";
/// How long a row watches a held member for a run that must not start.
const HELD_RUN_BOUND: Duration = Duration::from_secs(2);

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("parse identity")
}

fn durable_spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: id(name),
        profile: ProfileName::from("personal"),
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

/// A roster that lists these identities.
struct ListedRoster(&'static [&'static str]);

#[async_trait]
impl RosterProvider for ListedRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.0.iter().map(|name| durable_spec(name)).collect())
    }
}

/// The host's tools for one build.
struct HostTools;

#[async_trait]
impl AgentToolDispatcher for HostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef {
            audience: Default::default(),
            name: TOOL.into(),
            description: "Look up the workspace record.".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
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

/// A host customizer that declares [`TOOL`] and refuses the calls whose
/// 1-based ordinal is in `fail_calls`.
#[derive(Default)]
struct ToolCustomizer {
    calls: AtomicUsize,
    fail_calls: BTreeSet<usize>,
}

#[async_trait]
impl AgentCustomizer for ToolCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_calls.contains(&call) {
            return Err(CustomizerError::Io(format!("host refused build {call}")));
        }
        draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(HostTools));
        Ok(())
    }
}

/// A host customizer that declares [`TOOL`] only when its build context is
/// the runtime's real one: it carries the mob handle, and its active peers
/// list the identity being built. A host whose tools act on the mob and on
/// known peers refuses anything less: an empty peer list, or
/// `AgentRuntimeServices::empty()`.
#[derive(Default)]
struct RealContextCustomizer {
    calls: AtomicUsize,
}

#[async_trait]
impl AgentCustomizer for RealContextCustomizer {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if context.runtime_services.mob_handle().is_none() {
            return Err(CustomizerError::Io(
                "mob handle missing for the host's tools".to_string(),
            ));
        }
        if !context.active_peers.contains(&context.identity) {
            return Err(CustomizerError::Io(format!(
                "the build context's peers do not list {}: {:?}",
                context.identity, context.active_peers
            )));
        }
        draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(HostTools));
        Ok(())
    }
}

#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
}

impl CaptureClient {
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn last(&self) -> Option<String> {
        self.requests.lock().unwrap().last().cloned()
    }
    /// Whether a model request carried `text`.
    fn saw(&self, text: &str) -> bool {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.contains(text))
    }
    /// Every model request so far.
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl meerkat_client::LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(request).unwrap_or_default());
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }
    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }
    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

async fn boot(
    mob_id: &str,
    state: &Path,
    capture: CaptureClient,
    customizer: Arc<dyn AgentCustomizer>,
    mode: IdentityBootstrapMode,
) -> meerkat_meta::UnifiedRuntime {
    boot_listing(mob_id, state, capture, customizer, mode, &[MEMBER]).await
}

/// [`boot`] with a roster that lists `roster`.
async fn boot_listing(
    mob_id: &str,
    state: &Path,
    capture: CaptureClient,
    customizer: Arc<dyn AgentCustomizer>,
    mode: IdentityBootstrapMode,
    roster: &'static [&'static str],
) -> meerkat_meta::UnifiedRuntime {
    build_runtime(mob_id, state, capture, customizer, mode, roster, None).await
}

/// A boot whose roster lists `roster` and whose mob materializes every
/// seated member when it starts (`MemberRestore::Eager`): each is built
/// before the identity runtime is composed, as a member Core restores
/// because it owes work is.
async fn boot_restoring_all(
    mob_id: &str,
    state: &Path,
    capture: CaptureClient,
    customizer: Arc<dyn AgentCustomizer>,
    roster: &'static [&'static str],
) -> meerkat_meta::UnifiedRuntime {
    build_runtime(
        mob_id,
        state,
        capture,
        customizer,
        IdentityBootstrapMode::EagerMaterialize,
        roster,
        Some(MemberRestore::Eager),
    )
    .await
}

async fn build_runtime(
    mob_id: &str,
    state: &Path,
    capture: CaptureClient,
    customizer: Arc<dyn AgentCustomizer>,
    mode: IdentityBootstrapMode,
    roster: &'static [&'static str],
    restore: Option<MemberRestore>,
) -> meerkat_meta::UnifiedRuntime {
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(
            MobDefinition::from_toml(&MOB_TOML.replace("{MOB_ID}", mob_id)).expect("definition"),
        )
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(ListedRoster(roster)))
        .agent_customizer(customizer)
        .identity_bootstrap_mode(mode)
        .identity_runtime_instance_id(mob_id)
        .comms(true)
        .default_llm_client(Arc::new(capture));
    if let Some(restore) = restore {
        builder = builder.member_restore(restore);
    }
    Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime")
}

/// [`MEMBER`]'s status.
async fn member_status(runtime: &meerkat_meta::UnifiedRuntime) -> IdentityStatus {
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .status(&id(MEMBER))
        .await
        .expect("status")
}

/// Send `text` to [`MEMBER`] (`Queue`).
async fn send_member(
    runtime: &meerkat_meta::UnifiedRuntime,
    text: &str,
) -> Result<(), IdentityRuntimeError> {
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .send(
            &id(MEMBER),
            &meerkat_core::ContentInput::Text(text.to_string()),
        )
        .await
        .map(|_| ())
}

/// [`MEMBER`]'s live resolved tools: those its next model request carries.
/// The member must be materialized: the read never wakes it.
async fn resolved_tools(runtime: &meerkat_meta::UnifiedRuntime) -> Vec<String> {
    let session_id = member_status(runtime)
        .await
        .session_id
        .expect("member has a session");
    meerkat_meta::mob_handle_runtime::resolved_tools_for_session(
        runtime.mob_runtime().session_service(),
        MEMBER,
        session_id,
    )
    .await
    .expect("resolved tools")
    .tools
}

/// [`MEMBER`]'s `meta/identity/resolved_tools` answer, read through the
/// runtime's JSON-RPC dispatch as a host reads it.
async fn resolved_tools_answer(runtime: &meerkat_meta::UnifiedRuntime) -> serde_json::Value {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "resolved-tools",
        "method": "meta/identity/resolved_tools",
        "params": { "identity": MEMBER },
    });
    let reply = meerkat_meta::rpc::handle_unified_rpc_json(
        runtime,
        &request.to_string(),
        Duration::from_secs(30),
        None,
        None,
    )
    .await;
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("a JSON-RPC reply");
    assert!(
        reply.get("error").is_none(),
        "resolved_tools failed: {reply}"
    );
    reply["result"].clone()
}

/// The `tools` of a `meta/identity/resolved_tools` answer.
fn answered_tools(answer: &serde_json::Value) -> Vec<String> {
    answer["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer lists its tools: {answer}"))
        .iter()
        .map(|tool| tool.as_str().expect("a tool name").to_string())
        .collect()
}

/// Whether a rendered model request offers [`TOOL`]: the tool definitions it
/// carries name it, whatever its transcript mentions.
fn offers_tool(request: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(request)
        .is_ok_and(|request| request["tools"].to_string().contains(TOOL))
}

/// The visible names of the member's tool scope, as its last model-call
/// boundary left them.
async fn scope_visible_tools(runtime: &meerkat_meta::UnifiedRuntime) -> Vec<String> {
    let session_id = member_status(runtime)
        .await
        .session_id
        .expect("member has a session");
    let mut names = runtime
        .mob_runtime()
        .session_service()
        .expect("session service")
        .tool_scope_snapshot(&session_id)
        .await
        .expect("read the tool scope")
        .expect("the member's tool scope")
        .visible_names
        .into_iter()
        .map(meerkat_core::types::ToolName::into_string)
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// Whether meerkat holds the member's run starts, read from its runtime
/// (`None` while the member's session is not registered there: Core has not
/// built it).
async fn member_run_starts_held(runtime: &meerkat_meta::UnifiedRuntime) -> Option<bool> {
    let status = runtime.identity_runtime()?.status(&id(MEMBER)).await.ok()?;
    let session_id = status.session_id?;
    let machine = runtime
        .mob_runtime()
        .session_service()?
        .acquire_runtime_adapter(None)
        .expect("acquire the customizer restart fixture's runtime adapter")?;
    machine.run_starts_held_for_test(&session_id).await
}

async fn wait_for_turn(capture: &CaptureClient, want: usize, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while capture.count() < want {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(100)).await;
    }
}

/// Wait (bounded) until a model request carried `text`.
async fn wait_until_seen(capture: &CaptureClient, text: &str, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !capture.saw(text) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(100)).await;
    }
}

/// What a first boot left of [`MEMBER`]: its durable session, and the tools
/// its build had, read live after its turn.
struct FirstBoot {
    session_id: String,
    tools: Vec<String>,
}

/// Boot once (the identity seated fresh), deliver one turn, which builds the
/// member with its customizer tools, and stop cleanly.
async fn first_boot(mob_id: &str, state: &Path) -> FirstBoot {
    let capture = CaptureClient::default();
    let runtime = boot(
        mob_id,
        state,
        capture.clone(),
        Arc::new(ToolCustomizer::default()),
        IdentityBootstrapMode::EagerMaterialize,
    )
    .await;
    send_member(&runtime, "hello")
        .await
        .expect("send boot-1 turn");
    wait_for_turn(&capture, 1, "boot 1's turn").await;
    assert!(capture.last().expect("captured request").contains(TOOL));
    let tools = resolved_tools(&runtime).await;
    assert!(tools.contains(&TOOL.to_string()), "{tools:?}");
    let session_id = member_status(&runtime)
        .await
        .session_id
        .expect("member has a session")
        .to_string();
    // The build's report is written off Core's actor once the member is live.
    sleep(Duration::from_millis(500)).await;
    runtime.shutdown().await;
    FirstBoot { session_id, tools }
}

/// R110, R72, R97, R109: a clean restart over a member Core holds Dormant
/// builds nothing: no host build runs before the mob is built or lifted, no
/// pass seeds a tool record, and no member is materialized.
/// `meta/identity/resolved_tools` answers the tools of the member's last
/// build from its record, on its durable session, and the read wakes nothing
/// and calls no customizer. The first delivery wakes the member: the wake
/// prepares its build inline with one host build, its turn offers the tool,
/// and the build replaces the record, which the next clean restart answers.
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_restart_builds_no_member_and_answers_resolved_tools_from_its_record() {
    const MOB_ID: &str = "customizer-tools-restart-a";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let first = first_boot(MOB_ID, &state).await;

    {
        let capture = CaptureClient::default();
        let customizer = Arc::new(ToolCustomizer::default());
        let runtime = boot(
            MOB_ID,
            &state,
            capture.clone(),
            customizer.clone(),
            IdentityBootstrapMode::EagerMaterialize,
        )
        .await;
        assert_eq!(
            customizer.calls.load(Ordering::SeqCst),
            0,
            "the restart builds no member, so it calls the host's customize_build for none"
        );
        let status = member_status(&runtime).await;
        assert_eq!(status.state, IdentityLifecycleState::Active, "{status:?}");
        assert!(status.customizer_tools_pending.is_none(), "{status:?}");
        assert_eq!(
            member_run_starts_held(&runtime).await,
            None,
            "no member is materialized"
        );

        let answer = resolved_tools_answer(&runtime).await;
        assert_eq!(answer["tools_source"], "recorded", "{answer}");
        assert_eq!(answer["session_id"], first.session_id.as_str(), "{answer}");
        assert_eq!(answered_tools(&answer), first.tools, "{answer}");
        assert_eq!(answer["record_revision"], 1, "{answer}");
        assert_eq!(
            customizer.calls.load(Ordering::SeqCst),
            0,
            "the read calls no customizer"
        );
        assert_eq!(capture.count(), 0, "the read runs no turn");
        assert_eq!(
            member_run_starts_held(&runtime).await,
            None,
            "the read wakes nothing"
        );

        send_member(&runtime, "again")
            .await
            .expect("send boot-2 turn");
        wait_for_turn(&capture, 1, "boot 2's first turn").await;
        assert!(
            capture.last().expect("captured request").contains(TOOL),
            "the first turn after the restart offers the customizer tool"
        );
        assert_eq!(
            customizer.calls.load(Ordering::SeqCst),
            1,
            "the wake prepares the member's build once, inline"
        );
        let answer = resolved_tools_answer(&runtime).await;
        assert_eq!(answer["tools_source"], "live", "{answer}");
        assert_eq!(answered_tools(&answer), first.tools, "{answer}");
        // The build's report is written off Core's actor once the member is
        // live.
        sleep(Duration::from_millis(500)).await;
        runtime.shutdown().await;
    }

    let capture = CaptureClient::default();
    let customizer = Arc::new(ToolCustomizer::default());
    let runtime = boot(
        MOB_ID,
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    let answer = resolved_tools_answer(&runtime).await;
    assert_eq!(answer["tools_source"], "recorded", "{answer}");
    assert_eq!(
        answer["record_revision"], 2,
        "the wake's build replaced the record: {answer}"
    );
    assert_eq!(answer["session_id"], first.session_id.as_str(), "{answer}");
    assert_eq!(answered_tools(&answer), first.tools, "{answer}");
    assert_eq!(customizer.calls.load(Ordering::SeqCst), 0);
    assert_eq!(capture.count(), 0);
    runtime.shutdown().await;
}

/// R112, R109: after a clean restart the member is Dormant, and the build
/// preparation of its first delivery's wake fails, and so does the door's
/// one retry of it: the host refuses the build twice. Core refuses the wake
/// for the build preparation's retry and keeps the member Dormant, so the
/// delivery is refused, typed, with the host's reason, nothing is built and
/// nothing runs, and the identity's customizer tools read pending with that
/// reason. The next delivery retries the preparation and wakes the member:
/// its build is prepared with the tools, its turn offers them, and nothing
/// stays pending.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_build_preparation_keeps_the_member_dormant_until_the_next_delivery_builds_it() {
    const MOB_ID: &str = "customizer-tools-restart-b";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    first_boot(MOB_ID, &state).await;

    let capture = CaptureClient::default();
    let customizer = Arc::new(ToolCustomizer {
        calls: AtomicUsize::new(0),
        fail_calls: BTreeSet::from([1, 2]),
    });
    let runtime = boot(
        MOB_ID,
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        0,
        "the restart builds no member"
    );

    let refused = send_member(&runtime, "refused").await;
    match &refused {
        Err(IdentityRuntimeError::IdentityWakeRetryable { detail, .. }) => {
            assert!(detail.contains("host refused build 2"), "{detail}");
        }
        other => panic!("the delivery is refused retryable with the host's reason: {other:?}"),
    }
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        2,
        "the wake's preparation and the door's one retry of it"
    );
    let status = member_status(&runtime).await;
    let pending = status
        .customizer_tools_pending
        .expect("the refused preparation is recorded");
    assert!(
        pending.reason.contains("customize_build failed")
            && pending.reason.contains("host refused build 2"),
        "{pending:?}"
    );
    assert_eq!(
        member_run_starts_held(&runtime).await,
        None,
        "Core built nothing: the member stays Dormant"
    );
    assert_eq!(capture.count(), 0, "nothing ran");

    send_member(&runtime, "wake")
        .await
        .expect("the next delivery retries the preparation and builds the member");
    wait_for_turn(
        &capture,
        1,
        "the turn of the delivery that built the member",
    )
    .await;
    assert!(capture.last().expect("captured request").contains(TOOL));
    let status = member_status(&runtime).await;
    assert!(status.customizer_tools_pending.is_none(), "{status:?}");
    assert_eq!(
        member_run_starts_held(&runtime).await,
        Some(false),
        "the member runs unheld"
    );
    assert!(resolved_tools(&runtime).await.contains(&TOOL.to_string()));
    runtime.shutdown().await;
}

/// R111, R73: a restart that restores every seated member
/// (`MemberRestore::Eager`) builds the member before the identity runtime is
/// composed, so the preparer holds the build (Core stages the
/// `ToolsNotPublished` hold) without calling the host. The roster pass then
/// publishes the member's tools, once, with the runtime's real context, and
/// releases the hold: before any turn nothing is pending, nothing is held,
/// and the resolved tools list the customizer tool. The first turn offers
/// it, and the live member is not built again.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_built_before_the_composition_is_held_until_the_roster_pass_publishes_its_tools() {
    const MOB_ID: &str = "customizer-tools-restart-c";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    first_boot(MOB_ID, &state).await;

    let capture = CaptureClient::default();
    let customizer = Arc::new(RealContextCustomizer::default());
    let runtime = boot_restoring_all(
        MOB_ID,
        &state,
        capture.clone(),
        customizer.clone(),
        &[MEMBER],
    )
    .await;
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        1,
        "the roster pass's publication; the preparer held the build without calling the host"
    );
    let status = member_status(&runtime).await;
    assert!(
        status.customizer_tools_pending.is_none(),
        "the publication with the real context succeeded: {status:?}"
    );
    assert_eq!(
        member_run_starts_held(&runtime).await,
        Some(false),
        "the roster pass released the hold the build staged"
    );
    assert_eq!(capture.count(), 0, "no turn has run yet");
    assert!(
        resolved_tools(&runtime).await.contains(&TOOL.to_string()),
        "the restored member's resolved tools list its customizer tool before any turn"
    );

    send_member(&runtime, "after the restart")
        .await
        .expect("send boot-2 turn");
    wait_for_turn(&capture, 1, "boot 2's first turn").await;
    assert!(capture.last().expect("captured request").contains(TOOL));
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        1,
        "the live member is not built again"
    );
    runtime.shutdown().await;
}

/// R111, R112: the member is built before the composition and held, and the
/// roster pass's publication fails: the host refuses the build. The member
/// stays held, with the host's reason pending. Work Core admits for it
/// meanwhile is queued, and starts no run while the member is held. The
/// next delivery through Meta publishes the tools, which releases the hold:
/// the queued work and the delivery then run, and every model request offers
/// the tool.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_member_whose_publication_fails_starts_no_run_until_a_delivery_publishes_its_tools()
{
    const MOB_ID: &str = "customizer-tools-restart-l";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    first_boot(MOB_ID, &state).await;

    let capture = CaptureClient::default();
    let customizer = Arc::new(ToolCustomizer {
        calls: AtomicUsize::new(0),
        fail_calls: BTreeSet::from([1]),
    });
    let runtime = boot_restoring_all(
        MOB_ID,
        &state,
        capture.clone(),
        customizer.clone(),
        &[MEMBER],
    )
    .await;
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        1,
        "the roster pass's publication, refused"
    );
    let status = member_status(&runtime).await;
    let pending = status
        .customizer_tools_pending
        .expect("the refused publication is recorded");
    assert!(
        pending.reason.contains("host refused build 1"),
        "{pending:?}"
    );
    assert_eq!(
        member_run_starts_held(&runtime).await,
        Some(true),
        "the member stays held until its tools publish"
    );

    // Work Core admits for the held member waits behind the hold. The
    // member is an identity entry, so the work is the host's own (internal):
    // Core admits no external work to an entry.
    let handle = runtime.mob_handle();
    let member = handle
        .get_member(&meerkat_meta::member_comms_id::mob_member_id(MEMBER))
        .await
        .expect("Core's roster read")
        .expect("Core holds the member");
    tokio::time::timeout(
        Duration::from_secs(10),
        handle.submit_work_with_mode(
            member.agent_runtime_id,
            member.fence_token,
            meerkat_mob::WorkRef::new(),
            meerkat_mob::WorkSpec::new(QUEUED_WORK, meerkat_mob::WorkOrigin::Internal),
            meerkat_core::types::HandlingMode::Queue,
        ),
    )
    .await
    .expect("the submit is answered")
    .expect("Core admits the work and queues it");
    sleep(HELD_RUN_BOUND).await;
    assert_eq!(
        capture.count(),
        0,
        "the queued work started no run while the member was held"
    );

    send_member(&runtime, "after the hold")
        .await
        .expect("the delivery publishes the tools and is admitted");
    wait_until_seen(&capture, QUEUED_WORK, "the queued work's run").await;
    wait_until_seen(&capture, "after the hold", "the delivery's run").await;
    let requests = capture.requests();
    assert!(
        requests.iter().all(|request| offers_tool(request)),
        "every run after the release offers the customizer tool: {requests:?}"
    );
    let status = member_status(&runtime).await;
    assert!(status.customizer_tools_pending.is_none(), "{status:?}");
    assert_eq!(member_run_starts_held(&runtime).await, Some(false));
    runtime.shutdown().await;
}

/// Set in the child process of
/// [`a_member_whose_run_a_kill_left_open_continues_it_only_with_its_tools`]:
/// the state directory its first boot runs in.
const UNCLEAN_STOP_CHILD_STATE: &str = "META_CUSTOMIZER_TOOLS_UNCLEAN_STOP_CHILD_STATE";

/// R111, R73: a kill while the member's run is open (a no-shutdown exit: no
/// Stop is recorded, so the mob log stays Running) leaves work Core owes, so
/// the restart's boot plan restores the member while the mob is built,
/// before the identity runtime is composed, and the preparer holds the
/// build. The roster pass publishes the member's customizer tools with the
/// runtime's real context (the mob handle, the registered roster) and
/// releases the hold. Only then does the open run continue from its
/// committed round: every request it makes after the restart offers the
/// tool, and the host builds once.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_whose_run_a_kill_left_open_continues_it_only_with_its_tools() {
    const MOB_ID: &str = "customizer-tools-restart-d";
    if let Some(state) = std::env::var_os(UNCLEAN_STOP_CHILD_STATE) {
        // The child: the member's run commits a round and waits on its
        // model, then the process exits without shutdown.
        let model = WorkerModel::holding();
        let runtime = boot_spawning(
            MOB_ID,
            Path::new(&state),
            model.clone(),
            Arc::new(ToolCustomizer::default()),
            &[MEMBER],
        )
        .await;
        send_member(&runtime, &worker_turn(MEMBER))
            .await
            .expect("send the member its work");
        wait_for_round(&model, MEMBER).await;
        std::process::exit(0);
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "a_member_whose_run_a_kill_left_open_continues_it_only_with_its_tools",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(UNCLEAN_STOP_CHILD_STATE, &state)
        .status()
        .expect("run the first boot in a child process");
    assert!(child.success(), "the child's first boot failed: {child}");

    let model = WorkerModel::default();
    let customizer = Arc::new(RealContextCustomizer::default());
    let runtime = boot_spawning(MOB_ID, &state, model.clone(), customizer.clone(), &[MEMBER]).await;
    wait_for_round(&model, MEMBER).await;
    let continued = model.after_round_requests(MEMBER);
    assert!(
        continued.iter().all(|request| offers_tool(request)),
        "the open run continued only with the customizer tool: {continued:?}"
    );
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        1,
        "the roster pass's publication, with the real context; the preparer held the build \
         without calling the host"
    );
    let status = member_status(&runtime).await;
    assert!(status.customizer_tools_pending.is_none(), "{status:?}");
    assert_eq!(member_run_starts_held(&runtime).await, Some(false));
    // Once the member has run, its tool scope holds the same tools.
    assert_eq!(
        resolved_tools(&runtime).await,
        scope_visible_tools(&runtime).await
    );
    runtime.shutdown().await;
}

/// R121, R109, R73: a first boot seats the roster identity Dormant. The seat
/// records the host's build of the member's draft (one `customize_build`,
/// with the runtime's real context) and builds no member. Until a build is
/// recorded, `meta/identity/resolved_tools` answers the typed
/// `no_build_recorded` with the identity's session and no tools, and the
/// read wakes nothing and calls no customizer. The first delivery wakes the
/// member: the wake prepares its build inline (one more `customize_build`),
/// its turn offers the tool, and the answer is the full live list. After a
/// clean restart the record answers that same list, without a wake.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_never_built_answers_no_build_recorded_until_its_first_delivery_builds_it() {
    const MOB_ID: &str = "customizer-tools-restart-e";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");

    let (session_id, built) = {
        let capture = CaptureClient::default();
        let customizer = Arc::new(RealContextCustomizer::default());
        let runtime = boot(
            MOB_ID,
            &state,
            capture.clone(),
            customizer.clone(),
            IdentityBootstrapMode::LazyMaterialize,
        )
        .await;
        assert_eq!(
            customizer.calls.load(Ordering::SeqCst),
            1,
            "the seat builds the member's draft once, with the runtime's real context"
        );
        let status = member_status(&runtime).await;
        assert!(status.customizer_tools_pending.is_none(), "{status:?}");
        let session_id = status
            .session_id
            .expect("the seat binds the identity to a session")
            .to_string();
        assert_eq!(
            member_run_starts_held(&runtime).await,
            None,
            "the seat builds no member"
        );

        let answer = resolved_tools_answer(&runtime).await;
        assert_eq!(answer["tools_source"], "no_build_recorded", "{answer}");
        assert_eq!(answer["session_id"], session_id.as_str(), "{answer}");
        assert!(answered_tools(&answer).is_empty(), "{answer}");
        assert_eq!(
            customizer.calls.load(Ordering::SeqCst),
            1,
            "the read calls no customizer"
        );
        assert_eq!(capture.count(), 0, "the read runs no turn");
        assert_eq!(
            member_run_starts_held(&runtime).await,
            None,
            "the read wakes nothing"
        );

        send_member(&runtime, "first")
            .await
            .expect("the first delivery builds the member");
        wait_for_turn(&capture, 1, "the first delivery's turn").await;
        assert!(capture.last().expect("captured request").contains(TOOL));
        assert_eq!(
            customizer.calls.load(Ordering::SeqCst),
            2,
            "the wake prepares the member's build once, inline, with the real context"
        );
        let answer = resolved_tools_answer(&runtime).await;
        assert_eq!(answer["tools_source"], "live", "{answer}");
        assert_eq!(answer["session_id"], session_id.as_str(), "{answer}");
        let built = answered_tools(&answer);
        assert!(built.contains(&TOOL.to_string()), "{answer}");
        // The build's report is written off Core's actor once the member is
        // live.
        sleep(Duration::from_millis(500)).await;
        runtime.shutdown().await;
        (session_id, built)
    };

    let capture = CaptureClient::default();
    let customizer = Arc::new(RealContextCustomizer::default());
    let runtime = boot(
        MOB_ID,
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    let answer = resolved_tools_answer(&runtime).await;
    assert_eq!(answer["tools_source"], "recorded", "{answer}");
    assert_eq!(answer["session_id"], session_id.as_str(), "{answer}");
    assert_eq!(
        answered_tools(&answer),
        built,
        "the record answers the full list the build had: {answer}"
    );
    assert_eq!(answer["record_revision"], 1, "{answer}");
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        0,
        "the restart builds nothing, and the read calls no customizer"
    );
    assert_eq!(capture.count(), 0, "the read runs no turn");
    assert_eq!(
        member_run_starts_held(&runtime).await,
        None,
        "the read wakes nothing"
    );
    runtime.shutdown().await;
}

/// Set in the child process of
/// [`a_restored_member_the_roster_no_longer_lists_starts_no_run_before_it_is_retired`]:
/// the state directory its first boot runs in.
const DROPPED_CHILD_STATE: &str = "META_CUSTOMIZER_TOOLS_DROPPED_CHILD_STATE";
/// A member the first boot runs and the restart's roster no longer lists.
const DROPPED: &str = "personal:bob";

/// A kill while the run of a member is open leaves work Core owes, so the
/// restart's boot plan restores the member while the mob is built, although
/// the restart's roster no longer lists its identity. Nothing publishes that
/// member's customizer tools: the preparer holds its build, so its open run
/// does not continue before the roster pass retires it, and it asks its
/// model nothing after the restart. The listed member's run, left open by the
/// same kill and restored the same way, is the positive control: it does
/// continue after the restart and asks the same model.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_member_the_roster_no_longer_lists_starts_no_run_before_it_is_retired() {
    const MOB_ID: &str = "customizer-tools-restart-f";
    if let Some(state) = std::env::var_os(DROPPED_CHILD_STATE) {
        // The child: each member's run commits a round and waits on its
        // model, then the process exits without shutdown.
        let model = WorkerModel::holding();
        let runtime = boot_spawning(
            MOB_ID,
            Path::new(&state),
            model.clone(),
            Arc::new(ToolCustomizer::default()),
            &[MEMBER, DROPPED],
        )
        .await;
        send_member(&runtime, &worker_turn(MEMBER))
            .await
            .expect("send the listed member its work");
        runtime
            .identity_runtime()
            .expect("identity runtime")
            .send(
                &id(DROPPED),
                &meerkat_core::ContentInput::Text(worker_turn(DROPPED)),
            )
            .await
            .expect("send the member its work");
        wait_for_round(&model, MEMBER).await;
        wait_for_round(&model, DROPPED).await;
        std::process::exit(0);
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "a_restored_member_the_roster_no_longer_lists_starts_no_run_before_it_is_retired",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(DROPPED_CHILD_STATE, &state)
        .status()
        .expect("run the first boot in a child process");
    assert!(child.success(), "the child's first boot failed: {child}");

    let model = WorkerModel::default();
    let runtime = boot_spawning(
        MOB_ID,
        &state,
        model.clone(),
        Arc::new(ToolCustomizer::default()),
        &[MEMBER],
    )
    .await;
    let dropped_member = meerkat_meta::member_comms_id::mob_member_id(DROPPED);
    let members = runtime
        .mob_runtime()
        .handle()
        .list_members()
        .await
        .into_iter()
        .map(|member| member.agent_identity.to_string())
        .collect::<Vec<_>>();
    assert!(
        !members.contains(&dropped_member.to_string()),
        "the roster pass retired the member the roster no longer lists: {members:?}"
    );
    // The positive control: the listed member's open run continues after
    // the restart, so a restored open run does ask this model.
    wait_for_round(&model, MEMBER).await;
    sleep(Duration::from_secs(1)).await;
    let dropped_turn = worker_turn(DROPPED);
    let requests = model.requests();
    assert!(
        requests
            .iter()
            .all(|request| !request.contains(&dropped_turn)),
        "the restored member the roster no longer lists asked its model nothing after the \
         restart: {requests:?}"
    );
    runtime.shutdown().await;
}

/// Set in the child process of
/// [`a_host_adopted_member_the_roster_no_longer_lists_is_not_held_after_a_restart`]:
/// the state directory its first boot runs in.
const ADOPTED_CHILD_STATE: &str = "META_CUSTOMIZER_TOOLS_ADOPTED_CHILD_STATE";
/// A member the first boot runs, a host then adopts, and the restart's roster
/// no longer lists.
const ADOPTED: &str = "personal:carol";

/// A host adopts `name`'s live member through Core: Core then holds an
/// identity intent row for it and its reconciler drives it, so the roster
/// pass never retires it. The roster pass seats the member without building
/// it, so one turn (answered by `capture`'s model) builds it first. Waits
/// (bounded) until Core's reconciler reports it converged.
async fn adopt(
    runtime: &meerkat_meta::UnifiedRuntime,
    capture: &CaptureClient,
    mob_id: &str,
    name: &str,
) {
    let turns = capture.count();
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .send(
            &id(name),
            &meerkat_core::ContentInput::Text("before the adoption".to_string()),
        )
        .await
        .expect("send the turn that builds the member");
    wait_for_turn(capture, turns + 1, "the turn that builds the member").await;
    let handle = runtime.mob_handle();
    let core_identity = meerkat_meta::member_comms_id::roster_member_id_for_identity(name);
    let entry = handle
        .get_member(&core_identity)
        .await
        .expect("Core's roster read")
        .expect("a live member to adopt");
    let session_id = entry
        .bridge_session_id()
        .expect("the member's session")
        .clone();
    let request = meerkat_mob::AdoptMemberIdentityDeclaration {
        mob_id: meerkat_mob::MobId::from(mob_id),
        agent_identity: core_identity.clone(),
        request_id: meerkat_mob::IdentityAdoptionId::new("restart-adoption")
            .expect("valid adoption id"),
        precondition: meerkat_mob::IdentityAdoptionPrecondition::ExpectedAbsent,
        declaration_scope: meerkat_mob::IdentityDeclarationScopeId::new("restart-host")
            .expect("valid declaration scope"),
        declaration_revision: 1,
        session: meerkat_mob::DesiredSessionTarget {
            session_id: session_id.clone(),
            lineage_id: meerkat_core::SessionLineageId::for_session(&session_id),
            lineage_generation: meerkat_core::SessionGeneration::INITIAL,
            authority_policy: meerkat_mob::DesiredSessionAuthorityPolicy::RequireExisting,
        },
        member: meerkat_mob::IdentityProfileMemberDeclaration {
            profile_name: entry.role.clone(),
            profile_override: None,
            model_override: None,
            external_addressable_override: None,
            context: None,
            labels: Some(entry.labels.clone()),
            additional_instructions: None,
            system_prompt_override: None,
            tool_access_policy: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: None,
            required_env_keys: Vec::new(),
            required_local_callback_tools: Vec::new(),
            execution: meerkat_mob::DesiredExecution::ControllingSession,
        },
        wiring_custody: Default::default(),
        owned_wiring: Default::default(),
        convergence: meerkat_mob::IdentityConvergenceMode::Drain { max_wait_ms: 1_000 },
    };
    let adopted = handle
        .adopt_member_identity_declaration(request)
        .await
        .expect("Core adopts the live member");
    assert!(
        matches!(
            adopted.adoption,
            meerkat_mob::IdentityAdoptionOutcome::Adopted { .. }
        ),
        "{:?}",
        adopted.adoption
    );
    assert!(
        converged(&handle, &core_identity, Duration::from_secs(30)).await,
        "Core converges the adopted identity"
    );
}

/// Whether Core's reconciler reports `core_identity` converged within
/// `bound`: until then Core keeps the member's admission closed while it
/// seats the adopted material.
async fn converged(
    handle: &meerkat_mob::MobHandle,
    core_identity: &meerkat_mob::AgentIdentity,
    bound: Duration,
) -> bool {
    let deadline = Instant::now() + bound;
    loop {
        let observation = handle
            .identity_convergence_status(core_identity)
            .await
            .expect("Core's convergence status read");
        if let meerkat_mob::IdentityStoredObservation::Valid(status) = &observation
            && status.condition() == meerkat_mob::IdentityConvergenceCondition::Converged
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(50)).await;
    }
}

/// A host adopted a member's identity, and the restart's roster no longer
/// lists it. The restart restores every seated member while the mob is
/// built (`MemberRestore::Eager`), the adopted one as well, before the
/// identity runtime is composed. The roster pass never retires a
/// host-adopted identity and no roster publication covers it, so a hold on
/// its build would never be released: its build is not held, it holds no run
/// starts after the restart's bootstrap, and once Core's reconciler admits
/// work to it again, its runs start as before the restart.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_adopted_member_the_roster_no_longer_lists_is_not_held_after_a_restart() {
    const MOB_ID: &str = "customizer-tools-restart-g";
    if let Some(state) = std::env::var_os(ADOPTED_CHILD_STATE) {
        // The child: the first boot seats both members, builds one with a
        // turn and a host adopts it, then the process exits without
        // shutdown.
        let capture = CaptureClient::default();
        let runtime = boot_listing(
            MOB_ID,
            Path::new(&state),
            capture.clone(),
            Arc::new(ToolCustomizer::default()),
            IdentityBootstrapMode::EagerMaterialize,
            &[MEMBER, ADOPTED],
        )
        .await;
        adopt(&runtime, &capture, MOB_ID, ADOPTED).await;
        sleep(Duration::from_millis(500)).await;
        std::process::exit(0);
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "a_host_adopted_member_the_roster_no_longer_lists_is_not_held_after_a_restart",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ADOPTED_CHILD_STATE, &state)
        .status()
        .expect("run the first boot in a child process");
    assert!(child.success(), "the child's first boot failed: {child}");

    let runtime = boot_restoring_all(
        MOB_ID,
        &state,
        CaptureClient::default(),
        Arc::new(ToolCustomizer::default()),
        &[MEMBER],
    )
    .await;
    let handle = runtime.mob_handle();
    let adopted_member = meerkat_meta::member_comms_id::roster_member_id_for_identity(ADOPTED);
    let entry = handle
        .get_member(&adopted_member)
        .await
        .expect("Core's roster read")
        .expect("the roster pass leaves the host-adopted member");
    let session_id = entry
        .bridge_session_id()
        .expect("the adopted member's session")
        .clone();
    let machine = runtime
        .mob_runtime()
        .session_service()
        .expect("session service")
        .acquire_runtime_adapter(None)
        .expect("acquire the customizer restart fixture's runtime adapter")
        .expect("the runtime adapter");
    assert_eq!(
        machine.run_starts_held_for_test(&session_id).await,
        Some(false),
        "the host-adopted member the roster no longer lists holds no run starts after the \
         restart's bootstrap: nothing would ever release a hold on it"
    );
    runtime.shutdown().await;
}

/// Set in the child process of
/// [`a_host_adopted_member_converges_and_runs_promptly_after_a_crash_restart`]:
/// the state directory its first boot runs in.
const CRASH_ADOPTED_CHILD_STATE: &str = "META_CUSTOMIZER_TOOLS_CRASH_ADOPTED_CHILD_STATE";
/// How long after the restart's build returns Core may take to converge the
/// adopted identity again: a new process reclaims the identity lease a
/// crashed predecessor of its own holder left behind at once. The margin
/// covers a loaded host; waiting for that lease to expire takes about 30 s.
const CRASH_RESTART_CONVERGENCE_BOUND: Duration = Duration::from_secs(15);
/// How long the row observes the restart, so a slower convergence (the
/// predecessor's lease expiring on its own) is measured and logged too.
const CRASH_RESTART_OBSERVATION: Duration = Duration::from_secs(45);

/// UTC now, to the millisecond, for the row's timeline lines.
fn utc_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// A host adopted a member's identity, and the process then ended without
/// a shutdown (a crash): its identity lease is never released. In the next
/// process Core's reconciler must converge the adopted identity again
/// before it admits work. That process holds the lease under the same
/// holder, so it reclaims the crashed predecessor's lease at once instead
/// of waiting for it to expire: the identity converges within
/// [`CRASH_RESTART_CONVERGENCE_BOUND`] of the build, and work submitted to
/// it then runs. The row logs a UTC timeline (the child's exit, the
/// build's return, each poll, the first converged read, the first admitted
/// submit, the run) for up to [`CRASH_RESTART_OBSERVATION`].
#[tokio::test(flavor = "multi_thread")]
async fn a_host_adopted_member_converges_and_runs_promptly_after_a_crash_restart() {
    const MOB_ID: &str = "customizer-tools-restart-h";
    const WORK: &str = "work for the host-adopted member after a crash restart";
    if let Some(state) = std::env::var_os(CRASH_ADOPTED_CHILD_STATE) {
        // The child: the first boot seats both members, builds one with a
        // turn and a host adopts it, then the process exits without
        // shutdown.
        let capture = CaptureClient::default();
        let runtime = boot_listing(
            MOB_ID,
            Path::new(&state),
            capture.clone(),
            Arc::new(ToolCustomizer::default()),
            IdentityBootstrapMode::EagerMaterialize,
            &[MEMBER, ADOPTED],
        )
        .await;
        adopt(&runtime, &capture, MOB_ID, ADOPTED).await;
        eprintln!("[timeline] {} child adopted and converged", utc_now());
        sleep(Duration::from_millis(500)).await;
        eprintln!("[timeline] {} child exits without shutdown", utc_now());
        std::process::exit(0);
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "a_host_adopted_member_converges_and_runs_promptly_after_a_crash_restart",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CRASH_ADOPTED_CHILD_STATE, &state)
        .status()
        .expect("run the first boot in a child process");
    assert!(child.success(), "the child's first boot failed: {child}");
    eprintln!(
        "[timeline] {} child reaped; restart build starts",
        utc_now()
    );

    let capture = CaptureClient::default();
    let runtime = boot_listing(
        MOB_ID,
        &state,
        capture.clone(),
        Arc::new(ToolCustomizer::default()),
        IdentityBootstrapMode::LazyMaterialize,
        &[MEMBER],
    )
    .await;
    let built = Instant::now();
    eprintln!("[timeline] {} restart build returned", utc_now());
    let handle = runtime.mob_handle();
    let adopted_member = meerkat_meta::member_comms_id::roster_member_id_for_identity(ADOPTED);
    let member = handle
        .member(&adopted_member)
        .await
        .expect("the restart keeps the host-adopted member");

    // Observe: each poll reads Core's convergence status and, until a submit
    // is admitted, submits the work once.
    let mut converged_after: Option<Duration> = None;
    let mut admitted_after: Option<Duration> = None;
    let mut last_condition = String::new();
    while built.elapsed() < CRASH_RESTART_OBSERVATION
        && (converged_after.is_none() || admitted_after.is_none())
    {
        let observation = handle
            .identity_convergence_status(&adopted_member)
            .await
            .expect("Core's convergence status read");
        // When the status was read: the submit below can take a while.
        let read_at = built.elapsed();
        let condition = match &observation {
            meerkat_mob::IdentityStoredObservation::Valid(status) => {
                format!("{:?}", status.condition())
            }
            other => format!("{other:?}"),
        };
        let converged = matches!(
            &observation,
            meerkat_mob::IdentityStoredObservation::Valid(status)
                if status.condition() == meerkat_mob::IdentityConvergenceCondition::Converged
        );
        let submit = if admitted_after.is_none() {
            match tokio::time::timeout(
                Duration::from_secs(5),
                member.send(
                    meerkat_core::ContentInput::Text(WORK.to_string()),
                    meerkat_core::types::HandlingMode::Queue,
                ),
            )
            .await
            {
                Ok(Ok(_)) => {
                    admitted_after = Some(built.elapsed());
                    "admitted".to_string()
                }
                Ok(Err(error)) => format!("refused: {error}"),
                Err(_) => "unanswered".to_string(),
            }
        } else {
            "already admitted".to_string()
        };
        if converged && converged_after.is_none() {
            converged_after = Some(read_at);
        }
        eprintln!(
            "[timeline] {} poll read +{:.3}s condition={condition} submit={submit}",
            utc_now(),
            read_at.as_secs_f64()
        );
        last_condition = condition;
        sleep(Duration::from_millis(250)).await;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while admitted_after.is_some() && !capture.saw(WORK) && Instant::now() < deadline {
        sleep(Duration::from_millis(50)).await;
    }
    let ran = capture.saw(WORK);
    eprintln!(
        "[timeline] {} observed: converged after {converged_after:?}, admitted after \
         {admitted_after:?}, ran {ran}, last condition {last_condition}",
        utc_now()
    );
    runtime.shutdown().await;

    let converged_after = converged_after.unwrap_or_else(|| {
        panic!(
            "Core did not converge the adopted identity within {CRASH_RESTART_OBSERVATION:?} of \
             the restart's build (last condition {last_condition})"
        )
    });
    assert!(
        ran,
        "work submitted to the adopted member runs once Core admits it (admitted after \
         {admitted_after:?})"
    );
    assert!(
        converged_after <= CRASH_RESTART_CONVERGENCE_BOUND,
        "the restart reclaims its crashed predecessor's identity lease at once: Core converged \
         the adopted identity {converged_after:?} after the build, beyond \
         {CRASH_RESTART_CONVERGENCE_BOUND:?}"
    );
}

// ---------------------------------------------------------------------------
// Durable members spawned at runtime
// ---------------------------------------------------------------------------

/// The definition of the spawned-worker rows: the roster's profile, and a
/// durable worker profile a member's mob tools spawn from at runtime.
const SPAWNING_MOB_TOML: &str = r#"
[mob]
id = "{MOB_ID}"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
durable = true

[profiles.worker.tools]
comms = true
"#;

/// The durable workers spawned at runtime, which no roster lists.
const WORKERS: [&str; 3] = ["worker-1", "worker-2", "worker-3"];
/// What each run [`WorkerModel`] keeps open works on: a worker's, or a
/// roster member's.
const WORKER_TURN: &str = "WORKER-RUN-OSCAR-9";
/// The tool call each such run commits its first round with.
const WORKER_ROUND_CALL: &str = "call-worker-round";
/// Set in the child process of
/// [`durable_workers_spawned_at_runtime_continue_after_a_kill`]: the state
/// directory its first boot runs in.
const SPAWNED_CHILD_STATE: &str = "META_SPAWNED_DURABLE_WORKERS_CHILD_STATE";

fn worker_turn(worker: &str) -> String {
    format!("{WORKER_TURN} for {worker}")
}

/// The model of the rows whose runs a kill or a shutdown leaves open. A run
/// whose work carries [`WORKER_TURN`] first asks for a tool no catalog has
/// (refused per call), so its round commits; with `hold`, the call after
/// that round never answers, so the run stays open.
#[derive(Clone, Default)]
struct WorkerModel {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    hold: bool,
}

impl WorkerModel {
    fn holding() -> Self {
        Self {
            hold: true,
            ..Self::default()
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// The calls of `worker`'s run made after its committed round.
    fn after_round(&self, worker: &str) -> usize {
        self.after_round_requests(worker).len()
    }

    /// The requests of [`Self::after_round`].
    fn after_round_requests(&self, worker: &str) -> Vec<String> {
        let turn = worker_turn(worker);
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.contains(&turn) && request.contains(WORKER_ROUND_CALL))
            .cloned()
            .collect()
    }
}

impl meerkat_client::LlmClient for WorkerModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let rendered = serde_json::to_string(request).unwrap_or_default();
        self.requests.lock().unwrap().push(rendered.clone());
        let after_round = request
            .messages
            .iter()
            .any(|message| matches!(message, meerkat_core::Message::ToolResults { .. }));
        let call_tool = rendered.contains(WORKER_TURN) && !after_round;
        let held = self.hold && rendered.contains(WORKER_TURN) && after_round;
        let [usage, done] = llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            if call_tool {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
        );
        Box::pin(async_stream::stream! {
            if held {
                std::future::pending::<()>().await;
            }
            if call_tool {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: WORKER_ROUND_CALL.to_string(),
                    name: "no_such_tool".to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                });
            } else {
                yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// Wait (bounded) until `worker`'s run asked its model again after its
/// committed round.
async fn wait_for_round(model: &WorkerModel, worker: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while model.after_round(worker) == 0 {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {worker}'s run to ask its model after its round: {:?}",
            model.requests()
        );
        sleep(Duration::from_millis(50)).await;
    }
}

/// A runtime of the open-run rows over `state`, whose roster lists `roster`
/// and whose host builds with `customizer`.
async fn boot_spawning(
    mob_id: &str,
    state: &Path,
    model: WorkerModel,
    customizer: Arc<dyn AgentCustomizer>,
    roster: &'static [&'static str],
) -> meerkat_meta::UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&SPAWNING_MOB_TOML.replace("{MOB_ID}", mob_id))
                    .expect("definition"),
            )
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(roster)))
            .agent_customizer(customizer)
            .identity_bootstrap_mode(IdentityBootstrapMode::EagerMaterialize)
            .identity_runtime_instance_id(mob_id)
            .comms(true)
            .default_llm_client(Arc::new(model))
            .build(),
    )
    .await
    .expect("build the spawning runtime")
}

/// The first boot of the spawned-worker rows. The roster lists [`MEMBER`]
/// and [`DROPPED`]; [`WORKERS`] are spawned at runtime through the mob
/// handle from the durable worker profile, as a member's `mob_spawn_member`
/// tool spawns them: each with its work as its initial message, which the
/// spawn admits as the worker's first turn (internal work). The worker
/// profile is not externally addressable, so an external send to a worker
/// (`MemberHandle::send`) is refused. Each worker's run commits a tool round
/// and then waits on its model.
async fn boot_with_running_workers(mob_id: &str, state: &Path) -> meerkat_meta::UnifiedRuntime {
    let model = WorkerModel::holding();
    let runtime = boot_spawning(
        mob_id,
        state,
        model.clone(),
        Arc::new(ToolCustomizer::default()),
        &[MEMBER, DROPPED],
    )
    .await;
    let handle = runtime.mob_runtime().handle();
    for worker in WORKERS {
        let spec =
            meerkat_mob::SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from(worker))
                .with_initial_message(meerkat_core::ContentInput::Text(worker_turn(worker)));
        handle
            .spawn_spec(spec)
            .await
            .unwrap_or_else(|error| panic!("spawn {worker} with its work: {error}"));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while WORKERS.iter().any(|worker| model.after_round(worker) == 0) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for every worker's run to commit its round: {:?}",
            model.requests()
        );
        sleep(Duration::from_millis(50)).await;
    }
    runtime
}

/// The restart of the spawned-worker rows, whose roster no longer lists
/// [`DROPPED`]. Every worker is still an active member, and its open run asks
/// its model again exactly once, from its committed round, with nothing sent
/// to it: only the run the first boot left open can make that call. The
/// identity the roster dropped is retired.
async fn workers_continue_at_the_restart(mob_id: &str, state: &Path) {
    let model = WorkerModel::default();
    let runtime = boot_spawning(
        mob_id,
        state,
        model.clone(),
        Arc::new(ToolCustomizer::default()),
        &[MEMBER],
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while WORKERS.iter().any(|worker| model.after_round(worker) == 0) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for every worker's open run to continue: {:?}",
            model.requests()
        );
        sleep(Duration::from_millis(100)).await;
    }
    sleep(Duration::from_millis(500)).await;
    let members = runtime.mob_runtime().handle().list_members().await;
    for worker in WORKERS {
        let member = members
            .iter()
            .find(|member| member.agent_identity.as_str() == worker)
            .unwrap_or_else(|| panic!("{worker} is still a member: {members:?}"));
        assert_eq!(
            member.status,
            meerkat_mob::MobMemberStatus::Active,
            "{worker}: {:?}",
            member.error
        );
        assert_eq!(
            model.after_round(worker),
            1,
            "{worker}'s open run asks its model again once, from its committed round: {:?}",
            model.requests()
        );
        // The same run, not its input admitted again: the worker's turn
        // appears once in the resumed transcript.
        for request in model.after_round_requests(worker) {
            assert_eq!(
                request.matches(&worker_turn(worker)).count(),
                1,
                "{worker}'s resumed run carries its turn once: {request}"
            );
        }
    }
    let dropped = meerkat_meta::member_comms_id::mob_member_id(DROPPED);
    assert!(
        !members
            .iter()
            .any(|member| member.agent_identity == dropped),
        "the roster pass retired the identity the roster dropped: {members:?}"
    );
    runtime.shutdown().await;
}

/// A kill mid-run: the durable workers spawned at runtime, each in a run
/// waiting on its model after a committed round, come back after the kill.
/// The roster pass, whose roster never lists them, neither holds nor retires
/// them, and each run continues from its committed round. The identity the
/// roster really dropped is still retired.
#[tokio::test(flavor = "multi_thread")]
async fn durable_workers_spawned_at_runtime_continue_after_a_kill() {
    const MOB_ID: &str = "customizer-tools-restart-i";
    if let Some(state) = std::env::var_os(SPAWNED_CHILD_STATE) {
        // The child: the first boot, then an exit without shutdown, as a
        // kill leaves it.
        let _runtime = boot_with_running_workers(MOB_ID, Path::new(&state)).await;
        std::process::exit(0);
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "durable_workers_spawned_at_runtime_continue_after_a_kill",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SPAWNED_CHILD_STATE, &state)
        .status()
        .expect("run the first boot in a child process");
    assert!(child.success(), "the child's first boot failed: {child}");
    workers_continue_at_the_restart(MOB_ID, &state).await;
}

/// A graceful shutdown mid-run (SIGTERM): it suspends the open runs of the
/// durable workers spawned at runtime, and the next start revives the
/// workers and continues each run from its committed round. The roster pass
/// neither holds nor retires them; the identity the roster really dropped
/// is still retired.
#[tokio::test(flavor = "multi_thread")]
async fn durable_workers_spawned_at_runtime_continue_after_a_graceful_shutdown() {
    const MOB_ID: &str = "customizer-tools-restart-j";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    boot_with_running_workers(MOB_ID, &state)
        .await
        .shutdown()
        .await;
    workers_continue_at_the_restart(MOB_ID, &state).await;
}

/// An event log that records every event the runtime persists.
#[derive(Clone, Default)]
struct RecordedEventLog(Arc<std::sync::Mutex<Vec<meerkat_meta::PersistedEvent>>>);

impl RecordedEventLog {
    /// `(event id, event type)` of every agent event persisted for the
    /// member whose runtime alias names `member`, in persisted order.
    fn agent_events_of(&self, member: &str) -> Vec<(String, String)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|persisted| match &persisted.event {
                meerkat_meta::UnifiedEvent::Agent {
                    agent_id,
                    event_type,
                    ..
                } if agent_id.contains(member) => Some((persisted.id.clone(), event_type.clone())),
                _ => None,
            })
            .collect()
    }

    fn ids(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|persisted| persisted.id.clone())
            .collect()
    }
}

impl meerkat_meta::EventLogStore for RecordedEventLog {
    fn append_batch(
        &self,
        events: Vec<meerkat_meta::PersistedEvent>,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<(), meerkat_meta::unified_runtime::EventLogError>,
                > + Send
                + '_,
        >,
    > {
        let recorded = Arc::clone(&self.0);
        Box::pin(async move {
            recorded.lock().unwrap().extend(events);
            Ok(())
        })
    }

    fn query(
        &self,
        _query: meerkat_meta::EventQuery,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Vec<meerkat_meta::PersistedEvent>,
                        meerkat_meta::unified_runtime::EventLogError,
                    >,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// Runs a graceful shutdown left open reach the host's event log after the
/// restart resumes them at boot, through Meta's forwarded member events:
/// each durable worker's run from its `run_resumed` through its
/// `turn_completed` to its `run_completed`, and no event twice. The forwarder
/// subscribes a member when the machine state change of its wake makes it
/// live (a pass that starts watching the mob is followed by another at once),
/// and replays the woken actor's events from its first, so nothing between
/// the boot wake and the attach is missing; a member Core still holds
/// unmaterialized (the roster's identity, which owes no work) is asked
/// nothing meanwhile. The log's `batch_size: 1` keeps its intake buffer at 4
/// events, under one resumed run's burst: the drain waits for room rather
/// than dropping the rest.
#[tokio::test(flavor = "multi_thread")]
async fn runs_resumed_at_boot_reach_the_event_log_from_resume_to_terminal_once() {
    const MOB_ID: &str = "customizer-tools-restart-p";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    boot_with_running_workers(MOB_ID, &state)
        .await
        .shutdown()
        .await;

    let log = RecordedEventLog::default();
    let model = WorkerModel::default();
    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&SPAWNING_MOB_TOML.replace("{MOB_ID}", MOB_ID))
                    .expect("definition"),
            )
            .persistent_state(&state)
            .continuity_from_state_dir(&state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[MEMBER])))
            .agent_customizer(Arc::new(ToolCustomizer::default()))
            .identity_bootstrap_mode(IdentityBootstrapMode::EagerMaterialize)
            .identity_runtime_instance_id(MOB_ID)
            .comms(true)
            .default_llm_client(Arc::new(model.clone()))
            .event_log(meerkat_meta::EventLogConfig {
                store: Box::new(log.clone()),
                filter: None,
                batch_size: 1,
                flush_interval: Duration::from_millis(50),
            })
            .build(),
    )
    .await
    .expect("restart the spawning runtime");

    let completed = |worker: &str| {
        log.agent_events_of(worker)
            .iter()
            .any(|(_, kind)| kind == "run_completed")
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while !WORKERS.iter().all(|&worker| completed(worker)) {
        if Instant::now() >= deadline {
            let diagnostics = resumed_run_diagnostics(&runtime, &log, &model).await;
            panic!("every resumed run reaches the event log:{diagnostics}");
        }
        runtime
            .drain_mob_agent_events()
            .await
            .expect("drain the forwarded member events");
        sleep(Duration::from_millis(50)).await;
    }
    // Anything forwarded twice would land behind the terminal.
    sleep(Duration::from_millis(500)).await;
    runtime
        .drain_mob_agent_events()
        .await
        .expect("drain the forwarded member events");
    sleep(Duration::from_millis(200)).await;

    for worker in WORKERS {
        let kinds: Vec<String> = log
            .agent_events_of(worker)
            .into_iter()
            .map(|(_, kind)| kind)
            .collect();
        let resumed = kinds
            .iter()
            .position(|kind| kind == "run_resumed")
            .unwrap_or_else(|| panic!("{worker}'s run resumed: {kinds:?}"));
        let terminal = kinds
            .iter()
            .position(|kind| kind == "run_completed")
            .unwrap_or_else(|| panic!("{worker}'s run completed: {kinds:?}"));
        assert!(resumed < terminal, "{worker}: {kinds:?}");
        // Core publishes the turn's `turn_completed` before the run's
        // terminal, in the same burst as the turn's text.
        if !kinds[resumed..terminal]
            .iter()
            .any(|kind| kind == "turn_completed")
        {
            let diagnostics = resumed_run_diagnostics(&runtime, &log, &model).await;
            panic!("{worker}'s resumed run logs its turn_completed: {kinds:?}{diagnostics}");
        }
        assert_eq!(
            kinds.iter().filter(|kind| *kind == "run_completed").count(),
            1,
            "{worker}'s run reaches its terminal once: {kinds:?}"
        );
    }
    let ids = log.ids();
    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(distinct.len(), ids.len(), "no event is forwarded twice");
    runtime.shutdown().await;
}

/// Per worker, what tells an event lost on the way to the log from a run
/// that never resumed or never completed: the event types of the runtime's
/// module events (the drain hands them every event it takes, whatever the
/// log keeps), the model calls the run made after its committed round, the
/// member's status, and the event types the log holds. Each runtime read is
/// bounded, so a wedged runtime cannot turn the failure into a hang.
async fn resumed_run_diagnostics(
    runtime: &meerkat_meta::UnifiedRuntime,
    log: &RecordedEventLog,
    model: &WorkerModel,
) -> String {
    const READ_BOUND: Duration = Duration::from_secs(5);
    let module_events = tokio::time::timeout(READ_BOUND, runtime.module_events())
        .await
        .ok();
    let members = tokio::time::timeout(READ_BOUND, runtime.mob_runtime().handle().list_members())
        .await
        .ok();
    let mut diagnostics = String::new();
    for worker in WORKERS {
        let drained: Option<Vec<&str>> = module_events.as_ref().map(|events| {
            events
                .iter()
                .filter_map(|envelope| match &envelope.event {
                    meerkat_meta::UnifiedEvent::Agent {
                        agent_id,
                        event_type,
                        ..
                    } if agent_id.contains(worker) => Some(event_type.as_str()),
                    _ => None,
                })
                .collect()
        });
        let member = members.as_ref().map(|members| {
            members
                .iter()
                .find(|member| member.agent_identity.as_str() == worker)
                .map(|member| format!("{:?} {:?}", member.status, member.error))
        });
        let logged: Vec<String> = log
            .agent_events_of(worker)
            .into_iter()
            .map(|(_, kind)| kind)
            .collect();
        diagnostics.push_str(&format!(
            "\n{worker}: module events {drained:?}; calls after its round {}; \
             member {member:?}; event log {logged:?} (None: not read within {READ_BOUND:?})",
            model.after_round(worker)
        ));
    }
    diagnostics
}

// ---------------------------------------------------------------------------
// A member an agent spawns into its own mob, messaged across a restart
// ---------------------------------------------------------------------------

/// The definition of the agent-spawn row: the roster's profile carries the
/// mob tools, so the identity's own turn spawns from the durable worker
/// profile into its own mob.
const AGENT_SPAWN_MOB_TOML: &str = r#"
[mob]
id = "{MOB_ID}"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
mob = true

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
durable = true

[profiles.worker.tools]
comms = true
"#;

/// The identity whose own turns spawn the helper and message it.
const CREATOR: &str = "personal:creator";
/// The member the creator spawns into its own mob.
const HELPER: &str = "helper-1";
/// The host turn that asks the creator to spawn the helper.
const SPAWN_TURN: &str = "SPAWN-THE-HELPER";
/// The call id of the creator's spawn; only the creator's transcript holds
/// it.
const SPAWN_CALL: &str = "call-spawn-the-helper";
/// The host turn that asks the creator to message the helper, followed by
/// the message's tag.
const MESSAGE_TURN: &str = "MESSAGE-THE-HELPER";
/// The call id of the creator's message to the helper, followed by the tag.
const MESSAGE_CALL: &str = "call-message-the-helper";
/// The body the creator sends the helper, followed by the tag.
const HELPER_MESSAGE: &str = "HELLO-HELPER";
/// The tags of the creator's two messages.
const BEFORE_RESTART: &str = "before-restart";
const AFTER_RESTART: &str = "after-restart";

/// The model of the agent-spawn row, for the creator and the helper alike.
/// A request whose transcript holds [`SPAWN_TURN`] is the creator's: it
/// calls `mob_spawn_member` without `auto_wire_parent`, as an agent writes
/// it, until its transcript holds that call, and `send_message` to the
/// helper's peer id once for each [`MESSAGE_TURN`] its transcript holds.
/// Every other request that carries [`HELPER_MESSAGE`] is the helper's turn
/// on the creator's message, and is recorded. Each tool result a transcript
/// holds is recorded too, wherever it sits in the transcript.
#[derive(Clone)]
struct CreatorModel {
    mob_id: String,
    helper_peer_id: Arc<std::sync::Mutex<Option<String>>>,
    tool_results: Arc<std::sync::Mutex<BTreeMap<String, (bool, String)>>>,
    helper_turns: Arc<std::sync::Mutex<Vec<String>>>,
}

impl CreatorModel {
    fn new(mob_id: &str) -> Self {
        Self {
            mob_id: mob_id.to_string(),
            helper_peer_id: Arc::default(),
            tool_results: Arc::default(),
            helper_turns: Arc::default(),
        }
    }

    /// The result the creator's model received for `call`: whether it is an
    /// error, and its text.
    fn tool_result(&self, call: &str) -> Option<(bool, String)> {
        self.tool_results.lock().unwrap().get(call).cloned()
    }

    /// Whether the helper ran a turn on the creator's message tagged `tag`.
    fn helper_received(&self, tag: &str) -> bool {
        let message = format!("{HELPER_MESSAGE} {tag}");
        self.helper_turns
            .lock()
            .unwrap()
            .iter()
            .any(|turn| turn.contains(&message))
    }

    /// The creator's next call on a request whose transcript is
    /// `transcript`, or `None` when its turn ends: each call is made once.
    fn creator_call(&self, transcript: &str) -> Option<(String, &'static str, serde_json::Value)> {
        if !transcript.contains(SPAWN_CALL) {
            // No `runtime_mode`: a member holds spawn-profile scope over its
            // own mob, not manage scope, and Core admits a privileged spawn
            // argument (such as `runtime_mode`) only under manage scope. The
            // worker profile declares the helper turn-driven.
            return Some((
                SPAWN_CALL.to_string(),
                "mob_spawn_member",
                serde_json::json!({
                    "mob_id": self.mob_id,
                    "profile": "worker",
                    "member_id": HELPER
                }),
            ));
        }
        let tag = [BEFORE_RESTART, AFTER_RESTART].into_iter().find(|tag| {
            transcript.contains(&format!("{MESSAGE_TURN} {tag}"))
                && !transcript.contains(&format!("{MESSAGE_CALL}-{tag}"))
        })?;
        let peer_id = self.helper_peer_id.lock().unwrap().clone()?;
        Some((
            format!("{MESSAGE_CALL}-{tag}"),
            "send_message",
            serde_json::json!({
                "peer_id": peer_id,
                "body": format!("{HELPER_MESSAGE} {tag}"),
                "handling_mode": "queue"
            }),
        ))
    }
}

impl meerkat_client::LlmClient for CreatorModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let transcript = serde_json::to_string(&request.messages).unwrap_or_default();
        {
            let mut recorded = self.tool_results.lock().unwrap();
            for message in &request.messages {
                if let meerkat_core::Message::ToolResults { results, .. } = message {
                    for result in results {
                        recorded.insert(
                            result.tool_use_id.clone(),
                            (result.is_error, result.text_content()),
                        );
                    }
                }
            }
        }
        let call = if transcript.contains(SPAWN_TURN) {
            self.creator_call(&transcript)
        } else {
            if transcript.contains(HELPER_MESSAGE) {
                self.helper_turns.lock().unwrap().push(transcript.clone());
            }
            None
        };
        let [usage, done] = llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            if call.is_some() {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
        );
        Box::pin(async_stream::stream! {
            match call {
                Some((id, name, args)) => {
                    yield Ok(LlmEvent::ToolCallComplete {
                        id,
                        name: name.to_string(),
                        args,
                        meta: None,
                    });
                }
                None => {
                    yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// A runtime of the agent-spawn rows over `state`, whose roster lists the
/// creator.
async fn boot_agent_spawn(
    mob_id: &str,
    state: &Path,
    model: CreatorModel,
) -> meerkat_meta::UnifiedRuntime {
    boot_agent_spawn_with(mob_id, state, model, MemberRestore::default()).await
}

/// [`boot_agent_spawn`] with the host's `member_restore`.
async fn boot_agent_spawn_with(
    mob_id: &str,
    state: &Path,
    model: CreatorModel,
    restore: MemberRestore,
) -> meerkat_meta::UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&AGENT_SPAWN_MOB_TOML.replace("{MOB_ID}", mob_id))
                    .expect("definition"),
            )
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[CREATOR])))
            .identity_runtime_instance_id(mob_id)
            .member_restore(restore)
            .comms(true)
            .default_llm_client(Arc::new(model))
            .build(),
    )
    .await
    .expect("build the agent-spawn runtime")
}

/// Wait (bounded) until the helper is an active member of the mob, wired
/// to its creator: the spawn's wiring may settle after the spawn answers.
async fn assert_helper_wired_to_creator(runtime: &meerkat_meta::UnifiedRuntime, when: &str) {
    let creator = meerkat_meta::member_comms_id::roster_member_id_for_identity(CREATOR);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let members = runtime.mob_runtime().handle().list_members().await;
        let helper = members
            .iter()
            .find(|member| member.agent_identity.as_str() == HELPER);
        if let Some(helper) = helper
            && helper.status == meerkat_mob::MobMemberStatus::Active
            && helper.wired_to.contains(&creator)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{when}: the helper is an active member wired to its creator, Meta's default for \
             an agent's spawn: {helper:?}"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

/// Ask the creator, through its own turn, to message the helper with `tag`,
/// and wait until the helper ran a turn on that message.
async fn creator_messages_helper(
    runtime: &meerkat_meta::UnifiedRuntime,
    model: &CreatorModel,
    tag: &str,
) {
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text(format!("{MESSAGE_TURN} {tag}")),
        )
        .await
        .unwrap_or_else(|error| panic!("the creator's message turn ({tag}) commits: {error}"));
    let (failed, result) = model
        .tool_result(&format!("{MESSAGE_CALL}-{tag}"))
        .unwrap_or_else(|| panic!("the creator called send_message to the helper ({tag})"));
    assert!(!failed, "the creator's message ({tag}) is sent: {result}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !model.helper_received(tag) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the helper's turn on the creator's message ({tag})"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

/// M5: an identity's own turn spawns a long-lived member into its own mob
/// with `mob_spawn_member`, from a durable profile and without
/// `auto_wire_parent`, and the member is wired to it (Meta's default for an
/// agent's spawn), so the creator's message reaches it. After a graceful
/// restart the member is still an active member of the mob, wired to its
/// creator, and Core holds it Dormant: nothing materialized it at the
/// start. The creator's next message wakes it, and it runs a turn on that
/// message.
#[tokio::test(flavor = "multi_thread")]
async fn m5_a_member_an_agent_spawned_into_its_own_mob_is_messaged_by_its_creator_across_a_restart()
{
    const MOB_ID: &str = "customizer-tools-restart-m";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let helper = meerkat_mob::AgentIdentity::from(HELPER);
    let model = CreatorModel::new(MOB_ID);

    let runtime = boot_agent_spawn(MOB_ID, &state, model.clone()).await;
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text(SPAWN_TURN.to_string()),
        )
        .await
        .expect("the creator's spawn turn commits");
    let (spawn_failed, spawn_result) = model
        .tool_result(SPAWN_CALL)
        .expect("the creator's model received its spawn's result");
    assert!(
        !spawn_failed,
        "the creator's spawn succeeds: {spawn_result}"
    );
    assert_helper_wired_to_creator(&runtime, "after the spawn").await;
    // The helper's comms identity outlives the process, so its peer id
    // stays the creator's address for it after the restart.
    let endpoint = runtime
        .mob_handle()
        .member_peer_endpoint(&helper)
        .await
        .expect("read the helper's endpoint")
        .expect("the spawned helper has an endpoint");
    *model.helper_peer_id.lock().unwrap() = Some(endpoint.descriptor.peer_id.to_string());
    creator_messages_helper(&runtime, &model, BEFORE_RESTART).await;
    runtime.shutdown().await;
    drop(runtime);

    let runtime = boot_agent_spawn(MOB_ID, &state, model.clone()).await;
    assert_helper_wired_to_creator(&runtime, "after the restart").await;
    assert_eq!(
        runtime
            .mob_handle()
            .member_materialization_for_test(&helper),
        Some(meerkat_mob::machines::mob_machine::MemberMaterializationPhase::Dormant),
        "nothing materialized the helper at the start"
    );
    creator_messages_helper(&runtime, &model, AFTER_RESTART).await;
    assert_eq!(
        runtime
            .mob_handle()
            .member_materialization_for_test(&helper),
        Some(meerkat_mob::machines::mob_machine::MemberMaterializationPhase::Live),
        "the creator's message woke the helper"
    );
    runtime.shutdown().await;
}

/// The definition of the child mob the child-restore row creates: one
/// turn-driven worker profile.
fn child_mob_definition(child_id: &str) -> MobDefinition {
    serde_json::from_value(serde_json::json!({
        "id": child_id,
        "profiles": { "worker": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true }
        } }
    }))
    .expect("child definition")
}

/// A child mob created for an identity's session, as an agent's
/// `mob_create` creates one through the agent mob tools' state, restores its
/// members as the host's `member_restore` says, like the host's own mob: the
/// state hands the policy and the restore concurrency to every mob it
/// creates or resumes. Under the default a restart leaves the child mob's
/// member Dormant; under `MemberRestore::Eager` the start materializes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_mob_restores_its_members_as_the_hosts_member_restore_says() {
    const MOB_ID: &str = "customizer-tools-restart-n";
    let child_name = format!("{MOB_ID}-child");
    let worker = meerkat_mob::AgentIdentity::from("child-worker");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = CreatorModel::new(MOB_ID);

    // The creator runs a turn first, so its session is committed: a child mob
    // whose owner session is committed is restored running. Every handle the
    // first lifetime takes stays in this block: its identity runtime owns the
    // state directory's topology state until it is dropped, so a restart over
    // the same directory opens only once they are all gone.
    let child_id = {
        let runtime = boot_agent_spawn(MOB_ID, &state, model.clone()).await;
        let identities = runtime.identity_runtime().expect("identity runtime");
        identities
            .send_awaiting_commit(
                &id(CREATOR),
                &meerkat_core::ContentInput::Text("hello".to_string()),
            )
            .await
            .expect("the creator's turn commits");
        let owner = identities
            .status(&id(CREATOR))
            .await
            .expect("the creator's status")
            .session_id
            .expect("the creator has a session");
        let mobs = runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the agent mob tools' state");
        let child_id = mobs
            .mob_create_definition_with_owner_bridge_session(
                child_mob_definition(&child_name),
                owner,
                true,
                false,
            )
            .await
            .expect("create the child mob");
        mobs.mob_spawn(
            &child_id,
            ProfileName::from("worker"),
            worker.clone(),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("seat the child mob's worker");
        runtime.shutdown().await;
        child_id
    };

    for (restore, phase) in [
        (
            MemberRestore::OnDemand,
            meerkat_mob::machines::mob_machine::MemberMaterializationPhase::Dormant,
        ),
        (
            MemberRestore::Eager,
            meerkat_mob::machines::mob_machine::MemberMaterializationPhase::Live,
        ),
    ] {
        let runtime = boot_agent_spawn_with(MOB_ID, &state, model.clone(), restore).await;
        let mobs = runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the agent mob tools' state");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let current = match mobs.handle_for(&child_id).await {
                Ok(child) => child.member_materialization_for_test(&worker),
                Err(_) => None,
            };
            if current == Some(phase) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{restore:?}: the child mob's worker reads {phase:?} after the restart, not \
                 {current:?}"
            );
            sleep(Duration::from_millis(50)).await;
        }
        runtime.shutdown().await;
    }
}

// ---------------------------------------------------------------------------
// A respawned identity keeps the mobs its session owns
// ---------------------------------------------------------------------------

/// A host customizer that refuses its builds while `refuse` is set, after
/// answering the next `answers_left` of them: none, so it refuses at once,
/// or one, so it answers a respawn's seat and refuses the build after it.
#[derive(Default)]
struct SwitchedCustomizer {
    refuse: std::sync::atomic::AtomicBool,
    answers_left: AtomicUsize,
}

impl SwitchedCustomizer {
    /// Refuse every build after the next `answers`.
    fn refuse_after(&self, answers: usize) {
        self.answers_left.store(answers, Ordering::SeqCst);
        self.refuse.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl AgentCustomizer for SwitchedCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        if self.refuse.load(Ordering::SeqCst)
            && self
                .answers_left
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_err()
        {
            return Err(CustomizerError::Io(
                "the host refuses this build".to_string(),
            ));
        }
        Ok(())
    }
}

/// [`boot_agent_spawn`] with a host customizer.
async fn boot_agent_spawn_customized(
    mob_id: &str,
    state: &Path,
    model: CreatorModel,
    customizer: Arc<dyn AgentCustomizer>,
) -> meerkat_meta::UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&AGENT_SPAWN_MOB_TOML.replace("{MOB_ID}", mob_id))
                    .expect("definition"),
            )
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[CREATOR])))
            .agent_customizer(customizer)
            .identity_runtime_instance_id(mob_id)
            .comms(true)
            .default_llm_client(Arc::new(model))
            .build(),
    )
    .await
    .expect("build the agent-spawn runtime")
}

/// What the creator's session owns in the respawn rows: the child mob it
/// created, with one seated worker, and its implicit mob.
struct OwnedMobs {
    owner: meerkat_core::SessionId,
    child: meerkat_mob::MobId,
    implicit: meerkat_mob::MobId,
}

/// The worker seated in the child mob of the respawn rows.
const OWNED_WORKER: &str = "child-worker";

/// Commit one turn of the creator, then create for its session a child mob
/// (as an agent's `mob_create` creates one) with one seated worker, and the
/// session's implicit mob (as its first `delegate` creates it).
async fn creator_owns_mobs(runtime: &meerkat_meta::UnifiedRuntime, child_name: &str) -> OwnedMobs {
    let identities = runtime.identity_runtime().expect("identity runtime");
    identities
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text("hello".to_string()),
        )
        .await
        .expect("the creator's turn commits");
    let owner = identities
        .status(&id(CREATOR))
        .await
        .expect("the creator's status")
        .session_id
        .expect("the creator has a session");
    let mobs = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the agent mob tools' state");
    let child = mobs
        .mob_create_definition_with_owner_bridge_session(
            child_mob_definition(child_name),
            owner.clone(),
            true,
            false,
        )
        .await
        .expect("create the child mob");
    mobs.mob_spawn(
        &child,
        ProfileName::from("worker"),
        meerkat_mob::AgentIdentity::from(OWNED_WORKER),
        Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        None,
        None,
    )
    .await
    .expect("seat the child mob's worker");
    let implicit = mobs
        .get_or_create_implicit_mob_for_bridge_session(&owner.to_string(), "gpt-5.5")
        .await
        .expect("create the session's implicit mob");
    OwnedMobs {
        owner,
        child,
        implicit,
    }
}

/// What a runtime reads of the mobs the creator's session owns: whether the
/// child mob is managed, whether its worker is seated in it, and whether the
/// implicit mob is managed.
async fn owned_mobs_read(
    runtime: &meerkat_meta::UnifiedRuntime,
    owned: &OwnedMobs,
) -> (bool, bool, bool) {
    let mobs = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the agent mob tools' state");
    let worker = meerkat_mob::AgentIdentity::from(OWNED_WORKER);
    let (child, seated) = match mobs.handle_for(&owned.child).await {
        Ok(child) => (
            true,
            child
                .list_members()
                .await
                .iter()
                .any(|entry| entry.agent_identity == worker),
        ),
        Err(_) => (false, false),
    };
    let implicit = mobs.handle_for(&owned.implicit).await.is_ok();
    (child, seated, implicit)
}

/// The names of the per-mob store files (`mob--<id>.db`) under `dir`, at
/// any depth, sorted: what a restart keeps on disk of the mobs it restores.
fn mob_store_files(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Some(name) = path.file_name().and_then(|name| name.to_str())
                && name.starts_with("mob--")
                && name.ends_with(".db")
            {
                found.push(name.to_string());
            }
        }
    }
    found.sort();
    found
}

/// A respawn whose wake fails after the identity is seated (the host's
/// customizer answers the respawn's seat and refuses the build after it)
/// leaves the identity seated and not built. A restart over the same state
/// directory then still holds the child mob the identity's session owns,
/// with its seated worker, and the session's implicit mob; and the
/// identity's next delivery runs on the same session.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawn_whose_wake_fails_keeps_the_mobs_its_session_owns_across_a_restart() {
    const MOB_ID: &str = "customizer-tools-restart-t";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = CreatorModel::new(MOB_ID);

    // Every handle of the first lifetime stays in this block, as in the
    // child-restore row.
    let owned = {
        let customizer = Arc::new(SwitchedCustomizer::default());
        let runtime =
            boot_agent_spawn_customized(MOB_ID, &state, model.clone(), customizer.clone()).await;
        let owned = creator_owns_mobs(&runtime, &format!("{MOB_ID}-child")).await;
        assert_eq!(
            owned_mobs_read(&runtime, &owned).await,
            (true, true, true),
            "before the respawn the session's mobs are managed"
        );
        assert!(
            !mob_store_files(&state).is_empty(),
            "the session's mobs keep their stores under the state directory"
        );

        customizer.refuse_after(1);
        let respawn = runtime
            .identity_runtime()
            .expect("identity runtime")
            .respawn(&id(CREATOR))
            .await;
        assert!(
            respawn.is_err(),
            "the respawn's wake is refused by the host's build: {respawn:?}"
        );
        // Read here, asserted with the restart's read below.
        let seat_after_refused_wake = runtime.mob_handle().member_materialization_for_test(
            &meerkat_meta::member_comms_id::roster_member_id_for_identity(CREATOR),
        );
        assert_eq!(
            owned_mobs_read(&runtime, &owned).await,
            (true, true, true),
            "after the failed respawn, in the same process, the session's mobs are managed"
        );
        runtime.shutdown().await;
        (owned, seat_after_refused_wake)
    };
    let (owned, seat_after_refused_wake) = owned;

    let runtime = boot_agent_spawn_customized(
        MOB_ID,
        &state,
        model.clone(),
        Arc::new(SwitchedCustomizer::default()),
    )
    .await;
    // For a failure's message: the mobs the restart holds or quarantines
    // are not managed, and neither is a mob it tore down.
    let unmanaged = match runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the agent mob tools' state")
        .mob_inventory()
        .await
    {
        Ok(inventory) => format!(
            "held {:?}, quarantined {}, mob stores on disk {:?}",
            inventory.held,
            inventory.quarantined.len(),
            mob_store_files(&state)
        ),
        Err(error) => format!("inventory unread: {error}"),
    };
    assert_eq!(
        (
            seat_after_refused_wake,
            owned_mobs_read(&runtime, &owned).await
        ),
        (
            Some(meerkat_mob::machines::mob_machine::MemberMaterializationPhase::Dormant),
            (true, true, true)
        ),
        "the identity stayed seated, not built, after the refused wake; and after the restart: \
         (child mob managed, its worker seated, implicit mob managed); {unmanaged}"
    );
    let identities = runtime.identity_runtime().expect("identity runtime");
    identities
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text("hello again".to_string()),
        )
        .await
        .expect("the creator's next turn commits");
    assert_eq!(
        identities
            .status(&id(CREATOR))
            .await
            .expect("the creator's status")
            .session_id,
        Some(owned.owner.clone()),
        "the creator runs on the session it had"
    );
    assert_eq!(
        owned_mobs_read(&runtime, &owned).await,
        (true, true, true),
        "after the creator's next turn"
    );
    runtime.shutdown().await;
}

/// A respawn the host's customizer refuses at once is refused before
/// anything changes: the identity's member stays live at its generation,
/// and a restart over the same state directory still holds the child mob
/// the identity's session owns, with its seated worker, and the session's
/// implicit mob.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawn_refused_at_its_seats_build_changes_nothing() {
    const MOB_ID: &str = "customizer-tools-restart-y";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = CreatorModel::new(MOB_ID);
    let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(CREATOR);

    let (owned, after_refusal, generation_before) = {
        let customizer = Arc::new(SwitchedCustomizer::default());
        let runtime =
            boot_agent_spawn_customized(MOB_ID, &state, model.clone(), customizer.clone()).await;
        let owned = creator_owns_mobs(&runtime, &format!("{MOB_ID}-child")).await;
        let identities = runtime.identity_runtime().expect("identity runtime");
        let generation_before = identities
            .status(&id(CREATOR))
            .await
            .expect("the creator's status")
            .generation;

        customizer.refuse_after(0);
        let respawn = identities.respawn(&id(CREATOR)).await;
        assert!(
            respawn.is_err(),
            "the respawn is refused by the host's build: {respawn:?}"
        );
        let after_refusal = (
            runtime
                .mob_handle()
                .member_materialization_for_test(&member),
            identities
                .status(&id(CREATOR))
                .await
                .expect("the creator's status")
                .generation,
        );
        runtime.shutdown().await;
        (owned, after_refusal, generation_before)
    };

    let runtime = boot_agent_spawn_customized(
        MOB_ID,
        &state,
        model.clone(),
        Arc::new(SwitchedCustomizer::default()),
    )
    .await;
    assert_eq!(
        (after_refusal, owned_mobs_read(&runtime, &owned).await),
        (
            (
                Some(meerkat_mob::machines::mob_machine::MemberMaterializationPhase::Live),
                generation_before
            ),
            (true, true, true)
        ),
        "the refused respawn left the member live at its generation; and after the restart: \
         (child mob managed, its worker seated, implicit mob managed)"
    );
    runtime.shutdown().await;
}

/// A respawn whose wake fails, followed in the same process by a delivery
/// that wakes the identity: a restart over the same state directory still
/// holds the child mob the identity's session owns, with its seated worker,
/// and the session's implicit mob.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawn_whose_wake_fails_and_a_later_wake_keep_the_mobs_its_session_owns_across_a_restart()
 {
    const MOB_ID: &str = "customizer-tools-restart-v";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = CreatorModel::new(MOB_ID);

    let owned = {
        let customizer = Arc::new(SwitchedCustomizer::default());
        let runtime =
            boot_agent_spawn_customized(MOB_ID, &state, model.clone(), customizer.clone()).await;
        let owned = creator_owns_mobs(&runtime, &format!("{MOB_ID}-child")).await;
        let identities = runtime.identity_runtime().expect("identity runtime");

        customizer.refuse_after(1);
        let respawn = identities.respawn(&id(CREATOR)).await;
        assert!(
            respawn.is_err(),
            "the respawn's wake is refused by the host's build: {respawn:?}"
        );
        customizer.refuse.store(false, Ordering::SeqCst);
        identities
            .send_awaiting_commit(
                &id(CREATOR),
                &meerkat_core::ContentInput::Text("hello again".to_string()),
            )
            .await
            .expect("the creator's next turn wakes it and commits");
        assert_eq!(
            identities
                .status(&id(CREATOR))
                .await
                .expect("the creator's status")
                .session_id,
            Some(owned.owner.clone()),
            "the creator runs on the session it had"
        );
        runtime.shutdown().await;
        owned
    };

    let runtime = boot_agent_spawn_customized(
        MOB_ID,
        &state,
        model.clone(),
        Arc::new(SwitchedCustomizer::default()),
    )
    .await;
    assert_eq!(
        owned_mobs_read(&runtime, &owned).await,
        (true, true, true),
        "after the restart: (child mob managed, its worker seated, implicit mob managed)"
    );
    runtime.shutdown().await;
}

/// A respawn that completes keeps the identity on its session, and a
/// restart over the same state directory still holds the child mob the
/// session owns, with its seated worker, and the session's implicit mob.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawn_that_completes_keeps_the_mobs_its_session_owns_across_a_restart() {
    const MOB_ID: &str = "customizer-tools-restart-u";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = CreatorModel::new(MOB_ID);

    let owned = {
        let runtime = boot_agent_spawn_customized(
            MOB_ID,
            &state,
            model.clone(),
            Arc::new(SwitchedCustomizer::default()),
        )
        .await;
        let owned = creator_owns_mobs(&runtime, &format!("{MOB_ID}-child")).await;
        let identities = runtime.identity_runtime().expect("identity runtime");
        identities
            .respawn(&id(CREATOR))
            .await
            .expect("the respawn completes");
        assert_eq!(
            identities
                .status(&id(CREATOR))
                .await
                .expect("the creator's status")
                .session_id,
            Some(owned.owner.clone()),
            "the respawned creator runs on the session it had"
        );
        assert_eq!(
            owned_mobs_read(&runtime, &owned).await,
            (true, true, true),
            "after the respawn, in the same process"
        );
        runtime.shutdown().await;
        owned
    };

    let runtime = boot_agent_spawn_customized(
        MOB_ID,
        &state,
        model.clone(),
        Arc::new(SwitchedCustomizer::default()),
    )
    .await;
    assert_eq!(
        owned_mobs_read(&runtime, &owned).await,
        (true, true, true),
        "after the restart: (child mob managed, its worker seated, implicit mob managed)"
    );
    runtime.shutdown().await;
}

/// The label the answering customizer sets, with its answer's version.
const ANSWER_LABEL: &str = "customizer-answer";

/// A host customizer whose answer has a version the test switches: each
/// `customize_build` sets the draft's model, provider parameters, labels,
/// context and additional instructions from the current version (and, when
/// `prompts` is set, its system prompt), and each `after_create` is recorded
/// with the session and the context it is told.
#[derive(Default)]
struct AnsweringCustomizer {
    second: std::sync::atomic::AtomicBool,
    prompts: std::sync::atomic::AtomicBool,
    calls: AtomicUsize,
    told: std::sync::Mutex<Vec<(String, SessionCreatedContext)>>,
}

impl AnsweringCustomizer {
    /// The model, the temperature and the label of the answer in force.
    fn answer(&self) -> (&'static str, f32, &'static str) {
        if self.second.load(Ordering::SeqCst) {
            ("gpt-5.4", 0.75, "second")
        } else {
            ("gpt-5.5", 0.25, "first")
        }
    }
}

#[async_trait]
impl AgentCustomizer for AnsweringCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (model, temperature, label) = self.answer();
        draft.model = Some(model.to_string());
        draft.provider_params = Some(
            meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                temperature: Some(temperature),
                ..Default::default()
            },
        );
        draft
            .labels
            .insert(ANSWER_LABEL.to_string(), label.to_string());
        draft.app_context = Some(serde_json::json!({ "answer": label }));
        draft.additional_instructions = vec![format!("instructions of the {label} answer")];
        if self.prompts.load(Ordering::SeqCst) {
            draft.system_prompt = Some(format!("system prompt of the {label} answer"));
        }
        Ok(())
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        self.told
            .lock()
            .unwrap()
            .push((session_id.to_string(), context.clone()));
        Ok(())
    }
}

/// The model and the temperature one request names.
type RequestedModel = (String, Option<f32>);

/// A model that records the model and the temperature of each request.
#[derive(Clone, Default)]
struct RequestRecordingModel {
    requests: Arc<std::sync::Mutex<Vec<RequestedModel>>>,
}

impl meerkat_client::LlmClient for RequestRecordingModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push((request.model.clone(), request.temperature));
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }
    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }
    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// The value of [`ANSWER_LABEL`] on the creator's member in Core's roster.
async fn creator_answer_label(runtime: &meerkat_meta::UnifiedRuntime) -> Option<String> {
    let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(CREATOR);
    runtime
        .mob_runtime()
        .handle()
        .list_members()
        .await
        .into_iter()
        .find(|entry| entry.agent_identity == member)
        .and_then(|entry| entry.labels.get(ANSWER_LABEL).cloned())
}

/// The application context the creator's stored session holds: what its
/// last build stored in the session's build state.
async fn creator_stored_context(
    runtime: &meerkat_meta::UnifiedRuntime,
    session: &meerkat_core::types::SessionId,
) -> Option<serde_json::Value> {
    meerkat_mob::MobSessionService::load_persisted_session(
        &**runtime
            .mob_runtime()
            .session_service()
            .expect("the runtime's session service"),
        session,
    )
    .await
    .expect("the creator's session reads")
    .expect("the creator's session is stored")
    .try_build_state()
    .expect("the session's build state reads")
    .and_then(|state| state.app_context)
}

/// A respawn seats the member with the customizer's current answer. The
/// host's customizer answers one model, temperature and label until the
/// test switches it; the identity runs a turn, the answer is switched, and
/// the identity is respawned. The respawn asks the customizer twice (its
/// seat, then its build) and tells `after_create` once, of the session it
/// keeps, with the new answer's model and label; the member's roster label
/// is the new answer's; and the next turn's request carries the new
/// answer's temperature, and the model the kept session ran on. The kept
/// session keeps the application context its first build stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawn_seats_and_builds_the_member_with_the_customizers_current_answer() {
    const MOB_ID: &str = "customizer-tools-restart-x";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let customizer = Arc::new(AnsweringCustomizer::default());
    let model = RequestRecordingModel::default();
    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&AGENT_SPAWN_MOB_TOML.replace("{MOB_ID}", MOB_ID))
                    .expect("definition"),
            )
            .persistent_state(&state)
            .continuity_from_state_dir(&state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[CREATOR])))
            .agent_customizer(customizer.clone())
            .identity_runtime_instance_id(MOB_ID)
            .comms(true)
            .default_llm_client(Arc::new(model.clone()))
            .build(),
    )
    .await
    .expect("build the runtime");
    let identities = runtime.identity_runtime().expect("identity runtime");
    identities
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text("hello".to_string()),
        )
        .await
        .expect("the creator's first turn commits");
    let session = identities
        .status(&id(CREATOR))
        .await
        .expect("the creator's status")
        .session_id
        .expect("the creator has a session");
    assert_eq!(
        model.requests.lock().unwrap().last().cloned(),
        Some(("gpt-5.5".to_string(), Some(0.25))),
        "the first turn's request carries the first answer's model and temperature"
    );
    assert_eq!(
        creator_answer_label(&runtime).await.as_deref(),
        Some("first"),
        "the seated member carries the first answer's label"
    );
    assert_eq!(
        creator_stored_context(&runtime, &session).await,
        Some(serde_json::json!({ "answer": "first" })),
        "the session's first build stores the first answer's context"
    );

    customizer.second.store(true, Ordering::SeqCst);
    let calls_before = customizer.calls.load(Ordering::SeqCst);
    let told_before = customizer.told.lock().unwrap().len();
    identities
        .respawn(&id(CREATOR))
        .await
        .expect("the respawn completes");

    let respawn_calls = customizer.calls.load(Ordering::SeqCst) - calls_before;
    let label = creator_answer_label(&runtime).await;
    identities
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text("hello again".to_string()),
        )
        .await
        .expect("the creator's next turn commits");
    let (request_model, request_temperature) = model
        .requests
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the next turn reached the model");
    assert_eq!(
        identities
            .status(&id(CREATOR))
            .await
            .expect("the creator's status")
            .session_id,
        Some(session.clone()),
        "on the session the identity had"
    );
    // `after_create` is told by the lifecycle projection, with the lock
    // released: wait for it, bounded.
    let deadline = Instant::now() + Duration::from_secs(30);
    while customizer.told.lock().unwrap().len() == told_before {
        assert!(
            Instant::now() < deadline,
            "the respawn tells after_create of the session it keeps"
        );
        sleep(Duration::from_millis(50)).await;
    }
    sleep(Duration::from_millis(500)).await;
    let told = customizer.told.lock().unwrap()[told_before..]
        .iter()
        .map(|(session, context)| {
            (
                session.clone(),
                context.model.clone(),
                context.labels.get(ANSWER_LABEL).cloned(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(respawn_calls, 2, "the respawn's seat, then its build");
    assert_eq!(
        label.as_deref(),
        Some("second"),
        "labels: the respawned member carries the current answer's label"
    );
    assert_eq!(
        request_temperature,
        Some(0.75),
        "provider parameters: the respawned member runs with the current answer's temperature"
    );
    assert_eq!(
        request_model, "gpt-5.5",
        "model: the respawned member's request names the model its session ran on before the \
         respawn; the session is kept, and the current answer's model is the one after_create \
         is told"
    );
    assert_eq!(
        told,
        vec![(
            session.to_string(),
            "gpt-5.4".to_string(),
            Some("second".to_string())
        )],
        "after_create is told once, of the kept session, with the current answer's model and \
         label"
    );
    assert_eq!(
        creator_stored_context(&runtime, &session).await,
        Some(serde_json::json!({ "answer": "first" })),
        "context: the kept session holds the context its first build stored; a respawn's \
         builds on it store no other"
    );
    runtime.shutdown().await;
}

/// A respawn of a member that was seated and never built: the successor is
/// seated on the same session, which no build has created yet, and its
/// first build takes the customizer's current answer whole, as the first
/// build of a newly seated identity does: its system prompt, its additional
/// instructions and its context.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawn_of_a_member_never_built_builds_it_with_the_customizers_current_answer() {
    const MOB_ID: &str = "customizer-tools-restart-y";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let customizer = Arc::new(AnsweringCustomizer::default());
    customizer.prompts.store(true, Ordering::SeqCst);
    let capture = CaptureClient::default();
    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&AGENT_SPAWN_MOB_TOML.replace("{MOB_ID}", MOB_ID))
                    .expect("definition"),
            )
            .persistent_state(&state)
            .continuity_from_state_dir(&state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[CREATOR])))
            .agent_customizer(customizer.clone())
            .identity_runtime_instance_id(MOB_ID)
            .comms(true)
            .default_llm_client(Arc::new(capture.clone()))
            .build(),
    )
    .await
    .expect("build the runtime");
    let identities = runtime.identity_runtime().expect("identity runtime");
    let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(CREATOR);
    let dormant = Some(meerkat_mob::machines::mob_machine::MemberMaterializationPhase::Dormant);
    let deadline = Instant::now() + Duration::from_secs(30);
    while runtime
        .mob_handle()
        .member_materialization_for_test(&member)
        != dormant
    {
        assert!(
            Instant::now() < deadline,
            "the roster seats the creator Dormant"
        );
        sleep(Duration::from_millis(50)).await;
    }
    let session = identities
        .status(&id(CREATOR))
        .await
        .expect("the creator's status")
        .session_id
        .expect("the seated creator is bound to a session");
    assert_eq!(
        capture.count(),
        0,
        "nothing ran: the creator was never built"
    );

    customizer.second.store(true, Ordering::SeqCst);
    identities
        .respawn(&id(CREATOR))
        .await
        .expect("the respawn completes");
    identities
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text("hello".to_string()),
        )
        .await
        .expect("the creator's first turn commits");

    assert_eq!(
        identities
            .status(&id(CREATOR))
            .await
            .expect("the creator's status")
            .session_id,
        Some(session.clone()),
        "on the session the identity was seated on"
    );
    let request = capture
        .last()
        .expect("the creator's first turn reached the model");
    assert_eq!(
        (
            request.contains("system prompt of the second answer"),
            request.contains("system prompt of the first answer"),
            request.contains("instructions of the second answer"),
            request.contains("instructions of the first answer"),
        ),
        (true, false, true, false),
        "the first build takes the current answer's system prompt and instructions, and \
         nothing of the answer the first seat was composed with: (second prompt, first \
         prompt, second instructions, first instructions) in {request}"
    );
    assert_eq!(
        creator_stored_context(&runtime, &session).await,
        Some(serde_json::json!({ "answer": "second" })),
        "the first build stores the current answer's context"
    );
    runtime.shutdown().await;
}

/// The host turn the queued-prompt row's model holds open.
const HELD_TURN: &str = "HOLD-THIS-TURN-OPEN";
/// The prompt the queued-prompt row queues behind the held turn.
const QUEUED_PROMPT: &str = "PROMPT-QUEUED-BEHIND-THE-HELD-TURN";
/// The model's answer to [`QUEUED_PROMPT`]: a transcript that holds it has
/// run the prompt.
const QUEUED_ANSWER: &str = "ANSWER-TO-THE-QUEUED-PROMPT";

/// The model of the queued-prompt row. It holds the first request whose
/// transcript carries [`HELD_TURN`] open until the request is dropped, and
/// answers a request whose transcript carries [`QUEUED_PROMPT`] and not yet
/// its answer with [`QUEUED_ANSWER`], counting each such request: one run
/// of the queued prompt.
#[derive(Clone, Default)]
struct HoldingModel {
    hold_taken: Arc<std::sync::atomic::AtomicBool>,
    held: Arc<std::sync::atomic::AtomicBool>,
    queued_runs: Arc<AtomicUsize>,
    last: Arc<std::sync::Mutex<String>>,
}

impl meerkat_client::LlmClient for HoldingModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let transcript = serde_json::to_string(&request.messages).unwrap_or_default();
        let hold = transcript.contains(HELD_TURN) && !self.hold_taken.swap(true, Ordering::SeqCst);
        let answers_queued =
            !hold && transcript.contains(QUEUED_PROMPT) && !transcript.contains(QUEUED_ANSWER);
        if answers_queued {
            self.queued_runs.fetch_add(1, Ordering::SeqCst);
        }
        *self.last.lock().unwrap() = transcript;
        let held = Arc::clone(&self.held);
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            if hold {
                held.store(true, Ordering::SeqCst);
                // Never answered: the turn ends when its request is dropped.
                std::future::pending::<()>().await;
            }
            let delta = if answers_queued { QUEUED_ANSWER } else { "ok" };
            yield Ok(LlmEvent::TextDelta { delta: delta.to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }
    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }
    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// A prompt queued behind the identity's turn in flight when the identity
/// is respawned is not lost and does not run twice: the respawn ends the
/// turn in flight, the identity's successor runs the queued prompt once on
/// the session it keeps, and its answer is in the transcript of the
/// identity's next turn.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_queued_at_a_respawn_runs_once_on_the_respawned_identity() {
    const MOB_ID: &str = "customizer-tools-restart-w";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = HoldingModel::default();
    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&AGENT_SPAWN_MOB_TOML.replace("{MOB_ID}", MOB_ID))
                    .expect("definition"),
            )
            .persistent_state(&state)
            .continuity_from_state_dir(&state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[CREATOR])))
            .identity_runtime_instance_id(MOB_ID)
            .comms(true)
            .default_llm_client(Arc::new(model.clone()))
            .build(),
    )
    .await
    .expect("build the runtime");
    let identities = runtime.identity_runtime().expect("identity runtime");
    identities
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text("hello".to_string()),
        )
        .await
        .expect("the creator's first turn commits");
    let session = identities
        .status(&id(CREATOR))
        .await
        .expect("the creator's status")
        .session_id
        .expect("the creator has a session");

    identities
        .send(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text(HELD_TURN.to_string()),
        )
        .await
        .expect("the held turn is admitted");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !model.held.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the held turn to reach the model"
        );
        sleep(Duration::from_millis(50)).await;
    }
    identities
        .send(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text(QUEUED_PROMPT.to_string()),
        )
        .await
        .expect("the prompt is queued behind the held turn");
    assert_eq!(
        model.queued_runs.load(Ordering::SeqCst),
        0,
        "the queued prompt has not run before the respawn"
    );

    identities
        .respawn(&id(CREATOR))
        .await
        .expect("the respawn completes");

    let deadline = Instant::now() + Duration::from_secs(30);
    while model.queued_runs.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "the prompt queued at the respawn ran on the respawned identity"
        );
        sleep(Duration::from_millis(50)).await;
    }
    identities
        .send_awaiting_commit(
            &id(CREATOR),
            &meerkat_core::ContentInput::Text("and then".to_string()),
        )
        .await
        .expect("the creator's next turn commits");
    assert_eq!(
        model.queued_runs.load(Ordering::SeqCst),
        1,
        "the queued prompt ran once"
    );
    assert!(
        model.last.lock().unwrap().contains(QUEUED_ANSWER),
        "the next turn's transcript holds the queued prompt's answer"
    );
    assert_eq!(
        identities
            .status(&id(CREATOR))
            .await
            .expect("the creator's status")
            .session_id,
        Some(session),
        "on the session the identity had"
    );
    runtime.shutdown().await;
}

// ---------------------------------------------------------------------------
// A member restored without its comms identity
// ---------------------------------------------------------------------------

/// Set in the child processes of
/// [`a_member_restored_without_its_comms_identity_reads_broken`]: the state
/// directory, and the step the child runs.
const BROKEN_CHILD_STATE: &str = "META_BROKEN_MEMBER_CHILD_STATE";
const BROKEN_CHILD_STEP: &str = "META_BROKEN_MEMBER_CHILD_STEP";

/// One step of the broken-member row, in a child process whose comms
/// identity root (`XDG_STATE_HOME`, and `HOME` for macOS) its parent chose.
/// Each step's mob restores every seated member when it starts
/// (`MemberRestore::Eager`). `first` boots, builds the member with one turn
/// and stops; `control` restarts over the same comms identity root and reads
/// everything healthy; `empty` restarts over an empty one, so Core restores
/// the member broken, and reads it broken everywhere.
async fn broken_member_step(mob_id: &str, step: &str, state: &Path) {
    let capture = CaptureClient::default();
    let runtime = boot_restoring_all(
        mob_id,
        state,
        capture.clone(),
        Arc::new(ToolCustomizer::default()),
        &[MEMBER],
    )
    .await;
    if step == "first" {
        // The roster pass seats the member without building it: its turn
        // builds it, with its comms identity.
        send_member(&runtime, "hello")
            .await
            .expect("send the first step's turn");
        wait_for_turn(&capture, 1, "the first step's turn").await;
    }
    let status = runtime
        .identity_runtime()
        .expect("identity runtime")
        .status(&id(MEMBER))
        .await
        .expect("status");
    let member = meerkat_meta::member_comms_id::mob_member_id(MEMBER);
    let listed = runtime.mob_runtime().handle().list_members().await;
    let entry = listed
        .iter()
        .find(|entry| entry.agent_identity == member)
        .unwrap_or_else(|| panic!("the member is listed: {listed:?}"));
    let broken = runtime.broken_members().await;
    let bootstrap = runtime
        .identity_runtime()
        .expect("identity runtime")
        .identity_bootstrap_status();
    match step {
        "first" => {}
        "control" => {
            assert!(
                bootstrap.ready,
                "the identity bootstrap is ready: {bootstrap:?}"
            );
            assert_eq!(
                status.state,
                meerkat_meta::identity_first::IdentityLifecycleState::Active,
                "{status:?}"
            );
            assert_eq!(
                entry.status,
                meerkat_mob::MobMemberStatus::Active,
                "{:?}",
                entry.error
            );
            assert!(broken.is_empty(), "nothing is broken: {broken:?}");
        }
        "empty" => {
            assert_eq!(
                status.state,
                meerkat_meta::identity_first::IdentityLifecycleState::Broken,
                "status_identity reads the member broken: {status:?}"
            );
            assert_eq!(
                entry.status,
                meerkat_mob::MobMemberStatus::Broken,
                "the member list reads it broken"
            );
            assert!(
                broken
                    .iter()
                    .any(|(broken_member, _)| *broken_member == member),
                "the readiness read names it: {broken:?}"
            );
            let entry = bootstrap
                .identities
                .get(&id(MEMBER))
                .expect("the identity bootstrap tracks the member");
            assert!(
                !bootstrap.ready
                    && entry.state == meerkat_meta::identity_first::IdentityBootstrapState::Broken
                    && entry
                        .error
                        .as_deref()
                        .is_some_and(|error| error.contains("comms endpoint")),
                "the identity bootstrap reads not ready, with Core's reason: {bootstrap:?}"
            );
        }
        other => panic!("unknown step {other}"),
    }
    runtime.shutdown().await;
}

/// A member restored on a host whose comms identity directory is empty is
/// broken (its live endpoint cannot match the one its mob recorded), and it
/// reads so: `status_identity` and the member list say broken, and the
/// readiness read (`UnifiedRuntime::broken_members`, which `/healthz` and
/// `meta/status` report) names it. The control, a restart over the same
/// directory, reads all of them healthy.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_restored_without_its_comms_identity_reads_broken() {
    const MOB_ID: &str = "customizer-tools-restart-k";
    if let (Some(state), Ok(step)) = (
        std::env::var_os(BROKEN_CHILD_STATE),
        std::env::var(BROKEN_CHILD_STEP),
    ) {
        broken_member_step(MOB_ID, &step, Path::new(&state)).await;
        return;
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let keys = temp.path().join("comms-identity");
    let empty = temp.path().join("another-hosts-comms-identity");
    for (step, root) in [("first", &keys), ("control", &keys), ("empty", &empty)] {
        std::fs::create_dir_all(root).expect("the comms identity root");
        let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "a_member_restored_without_its_comms_identity_reads_broken",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(BROKEN_CHILD_STATE, &state)
            .env(BROKEN_CHILD_STEP, step)
            .env("XDG_STATE_HOME", root)
            .env("HOME", root)
            .status()
            .expect("run the step in a child process");
        assert!(child.success(), "the {step} step failed: {child}");
    }
}

// ---------------------------------------------------------------------------
// A seated member Core cannot wake
// ---------------------------------------------------------------------------

/// Set in the child processes of
/// [`a_seated_member_core_cannot_wake_reads_broken_after_its_refused_wake`]:
/// the state directory, and the step the child runs.
const UNWAKEABLE_CHILD_STATE: &str = "META_UNWAKEABLE_MEMBER_CHILD_STATE";
const UNWAKEABLE_CHILD_STEP: &str = "META_UNWAKEABLE_MEMBER_CHILD_STEP";

/// One step of the unwakeable-member row, in a child process whose comms
/// identity root (`XDG_STATE_HOME`, and `HOME` for macOS) its parent chose.
/// Each step's mob restores on demand (the default `member_restore`).
/// `first` builds the member with one turn and stops; `empty` restarts over
/// an empty comms identity root, so the member is seated and reads active
/// until a delivery asks Core to build it, which Core refuses for good, and
/// from then on it reads broken.
async fn unwakeable_member_step(mob_id: &str, step: &str, state: &Path) {
    let capture = CaptureClient::default();
    let runtime = boot(
        mob_id,
        state,
        capture.clone(),
        Arc::new(ToolCustomizer::default()),
        IdentityBootstrapMode::EagerMaterialize,
    )
    .await;
    match step {
        "first" => {
            send_member(&runtime, "hello")
                .await
                .expect("send the first step's turn");
            wait_for_turn(&capture, 1, "the first step's turn").await;
        }
        "empty" => {
            // Seated and not built: nothing is broken before a wake.
            assert_eq!(
                member_status(&runtime).await.state,
                IdentityLifecycleState::Active,
                "the seated member reads active before its wake"
            );
            let refused = send_member(&runtime, "wake").await;
            assert!(
                matches!(refused, Err(IdentityRuntimeError::AdmissionFailed { .. })),
                "Core refuses the wake for good: the send answers the final typed refusal, \
                 not a retryable one: {refused:?}"
            );
            assert_eq!(capture.count(), 0, "nothing ran");
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let status = member_status(&runtime).await;
                if status.state == IdentityLifecycleState::Broken {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "the refused wake leaves the member broken: {status:?}"
                );
                sleep(Duration::from_millis(100)).await;
            }
            let member = meerkat_meta::member_comms_id::mob_member_id(MEMBER);
            let broken = runtime.broken_members().await;
            assert!(
                broken
                    .iter()
                    .any(|(broken_member, _)| *broken_member == member),
                "Core holds the member broken, as the readiness read names it: {broken:?}"
            );
        }
        other => panic!("unknown step {other}"),
    }
    runtime.shutdown().await;
}

/// Core-dependent (Core records Broken through its machine on a final wake
/// refusal, design rows R2/F4): a seated member Core cannot build reads
/// broken after its refused wake, end to end through Meta. The member is
/// seated over a host whose comms identity directory is empty, so it reads
/// active while nothing builds it; the first delivery asks Core to build it,
/// Core refuses the wake for good and holds the member Broken, and Meta's
/// status and readiness read it broken from Core's broken members.
#[tokio::test(flavor = "multi_thread")]
async fn a_seated_member_core_cannot_wake_reads_broken_after_its_refused_wake() {
    const MOB_ID: &str = "customizer-tools-restart-o";
    if let (Some(state), Ok(step)) = (
        std::env::var_os(UNWAKEABLE_CHILD_STATE),
        std::env::var(UNWAKEABLE_CHILD_STEP),
    ) {
        unwakeable_member_step(MOB_ID, &step, Path::new(&state)).await;
        return;
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let keys = temp.path().join("comms-identity");
    let empty = temp.path().join("another-hosts-comms-identity");
    for (step, root) in [("first", &keys), ("empty", &empty)] {
        std::fs::create_dir_all(root).expect("the comms identity root");
        let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "a_seated_member_core_cannot_wake_reads_broken_after_its_refused_wake",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(UNWAKEABLE_CHILD_STATE, &state)
            .env(UNWAKEABLE_CHILD_STEP, step)
            .env("XDG_STATE_HOME", root)
            .env("HOME", root)
            .status()
            .expect("run the step in a child process");
        assert!(child.success(), "the {step} step failed: {child}");
    }
}

// ---------------------------------------------------------------------------
// A governed input after a clean restart
// ---------------------------------------------------------------------------

/// The second roster identity of the governed-input row. The row's topology
/// wires it to [`MEMBER`], so every build context carries a managed edge.
const GOVERNED_PEER: &str = "personal:dana";
/// The governed input the row sends after the restart.
const GOVERNED_INPUT: &str = "the first governed input after the restart";
/// What the row's binder answers every bind with.
const BIND_REFUSED: &str = "the host binds nothing in this row";
/// Work whose turn [`ToolTurnModel`] answers with one call of [`TOOL`].
const TOOL_TURN: &str = "LOOKUP-THE-WORKSPACE-RECORD";
/// The id of that call.
const TOOL_TURN_CALL: &str = "call-lookup-the-workspace-record";

/// The governed-input row's topology: one managed edge, [`MEMBER`] to
/// [`GOVERNED_PEER`].
struct PairTopology;

#[async_trait]
impl TopologyProvider for PairTopology {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        Ok(vec![edge(MEMBER, GOVERNED_PEER)])
    }
}

/// The managed edge between `a` and `b`.
fn edge(a: &str, b: &str) -> ManagedPeerEdge {
    ManagedPeerEdge::new(id(a), id(b)).expect("a managed edge")
}

/// A managed edge as [`BuildInputs`] holds one: its ends in canonical order.
fn edge_ends(managed: &ManagedPeerEdge) -> (AgentIdentity, AgentIdentity) {
    (managed.a().clone(), managed.b().clone())
}

/// What one `customize_build` was handed: its context (the managed edges
/// as a set, and whether it carried the runtime's services), the roster row,
/// and the base draft in its serialized form.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildInputs {
    identity: AgentIdentity,
    active_peers: Vec<AgentIdentity>,
    managed_edges: BTreeSet<(AgentIdentity, AgentIdentity)>,
    runtime_services: bool,
    spec: DurableAgentSpec,
    draft: serde_json::Value,
}

impl BuildInputs {
    fn of(context: &AgentBuildContext, spec: &DurableAgentSpec, draft: &AgentBuildDraft) -> Self {
        Self {
            identity: context.identity.clone(),
            active_peers: context.active_peers.clone(),
            managed_edges: context.managed_edges.iter().map(edge_ends).collect(),
            runtime_services: context.runtime_services.mob_handle().is_some(),
            spec: spec.clone(),
            draft: serde_json::to_value(draft).expect("the base draft serializes"),
        }
    }
}

/// One `customize_build`: what it was handed, and the tag of the [`TOOL`]
/// handler it registered.
#[derive(Debug, Clone)]
struct BuildCall {
    inputs: BuildInputs,
    tag: String,
}

/// A host customizer that records every `customize_build` and counts every
/// `after_create`. Each build registers its own [`TOOL`] handler, tagged
/// `{boot}-{n}` for the customizer's `n`th build, and a handler that serves
/// a call records its tag in `served`, which the row's boots share: a tool
/// call names the build whose handler served it.
struct RecordingCustomizer {
    boot: &'static str,
    builds: std::sync::Mutex<Vec<BuildCall>>,
    after_creates: AtomicUsize,
    served: Arc<std::sync::Mutex<Vec<String>>>,
}

impl RecordingCustomizer {
    fn new(boot: &'static str, served: &Arc<std::sync::Mutex<Vec<String>>>) -> Self {
        Self {
            boot,
            builds: std::sync::Mutex::new(Vec::new()),
            after_creates: AtomicUsize::new(0),
            served: Arc::clone(served),
        }
    }

    fn builds(&self) -> Vec<BuildCall> {
        self.builds.lock().unwrap().clone()
    }

    fn after_creates(&self) -> usize {
        self.after_creates.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AgentCustomizer for RecordingCustomizer {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        let inputs = BuildInputs::of(context, spec, draft);
        let tag = {
            let mut builds = self.builds.lock().unwrap();
            let tag = format!("{}-{}", self.boot, builds.len() + 1);
            builds.push(BuildCall {
                inputs,
                tag: tag.clone(),
            });
            tag
        };
        draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(TaggedTools {
            tag,
            served: Arc::clone(&self.served),
        }));
        Ok(())
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        _session_id: &meerkat_core::types::SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        self.after_creates.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// [`TOOL`] as one build registered it: its handler records the build's tag
/// in `served` when it serves a call, and answers with the tag.
struct TaggedTools {
    tag: String,
    served: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentToolDispatcher for TaggedTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        HostTools.tools()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.served.lock().unwrap().push(self.tag.clone());
        Ok(meerkat_core::ToolResult {
            host_metadata: Default::default(),
            tool_use_id: call.id.to_string(),
            content: vec![meerkat_core::ContentBlock::Text {
                text: self.tag.clone(),
            }],
            is_error: false,
            settlement_failures: Vec::new(),
        }
        .into())
    }
}

/// The controller [`ToolTurnModel`] reports, as a governed member's
/// selection owner does: a composition that pins the member's controller
/// can then bind an input for it.
fn tool_turn_controller() -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: "gpt-5.5".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm": "customizer-restart", "account": "tool-turn-controller"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "tool-turn-profile".into(),
        "tool-turn-transport".into(),
    )
}

/// The model of the governed-input row. It records every request. A request
/// whose work carries [`TOOL_TURN`] and holds no tool result yet is answered
/// with one call of [`TOOL`]; every other request is answered with text.
#[derive(Clone, Default)]
struct ToolTurnModel {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
}

impl ToolTurnModel {
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl meerkat_client::LlmClient for ToolTurnModel {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(tool_turn_controller())
    }
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let rendered = serde_json::to_string(request).unwrap_or_default();
        self.requests.lock().unwrap().push(rendered.clone());
        let answered = request
            .messages
            .iter()
            .any(|message| matches!(message, meerkat_core::Message::ToolResults { .. }));
        let call_tool = rendered.contains(TOOL_TURN) && !answered;
        let [usage, done] = llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            if call_tool {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
        );
        Box::pin(async_stream::stream! {
            if call_tool {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: TOOL_TURN_CALL.to_string(),
                    name: TOOL.to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                });
            } else {
                yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// What one bind of [`RefusingBinder`] was handed: the member's session and
/// the controller its session owner pinned, and how many builds the host's
/// customizer had prepared in this process by then.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HandedBind {
    session_id: meerkat_core::types::SessionId,
    controller: meerkat_core::ControllerModelSelection,
    builds: usize,
}

/// The trusted host's binder of the governed-input rows. It binds nothing: it
/// records what each bind is handed, and refuses, so nothing is submitted.
struct RefusingBinder {
    customizer: Arc<RecordingCustomizer>,
    binds: std::sync::Mutex<Vec<HandedBind>>,
}

impl RefusingBinder {
    fn new(customizer: Arc<RecordingCustomizer>) -> Self {
        Self {
            customizer,
            binds: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn binds(&self) -> Vec<HandedBind> {
        self.binds.lock().unwrap().clone()
    }

    /// How many builds the host's customizer had prepared at each bind.
    fn builds_at_bind(&self) -> Vec<usize> {
        self.binds().iter().map(|bind| bind.builds).collect()
    }
}

#[async_trait]
impl GovernedInputBinder for RefusingBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        _input: meerkat_runtime::Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        let handed = HandedBind {
            session_id: context.session_id.clone(),
            controller: context.controller.selection().clone(),
            builds: self.customizer.builds().len(),
        };
        self.binds.lock().unwrap().push(handed);
        Err(GovernedBindRefusal::denied(BIND_REFUSED).into())
    }

    fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {
        panic!("nothing is submitted, so no admission is left unresolved");
    }
}

/// A boot of the governed-input row over `state`: the roster lists
/// [`MEMBER`] and [`GOVERNED_PEER`], [`PairTopology`] wires them, and the
/// host builds with `customizer`.
async fn boot_governed(
    mob_id: &str,
    state: &Path,
    model: ToolTurnModel,
    customizer: Arc<RecordingCustomizer>,
) -> meerkat_meta::UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&MOB_TOML.replace("{MOB_ID}", mob_id))
                    .expect("definition"),
            )
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[MEMBER, GOVERNED_PEER])))
            .topology_provider(Arc::new(PairTopology))
            .agent_customizer(customizer)
            .identity_bootstrap_mode(IdentityBootstrapMode::EagerMaterialize)
            .identity_runtime_instance_id(mob_id)
            .comms(true)
            .default_llm_client(Arc::new(model))
            .build(),
    )
    .await
    .expect("build the governed-input row's runtime")
}

/// Wait (bounded) until `ready` holds.
async fn wait_until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(100)).await;
    }
}

/// The lazy restore contract of Meta 0.9.6, for a governed input. Meta
/// 0.9.5 ran `customize_build` for every roster identity at a restart,
/// before the mob was restored, built every member at start, and called
/// `after_create` after every build, wakes included. Since 0.9.6 a governed
/// input after a clean restart wakes its member the way an ordinary
/// delivery does
/// ([`a_clean_restart_builds_no_member_and_answers_resolved_tools_from_its_record`]):
///
/// - before any input the restart has called `customize_build` for no
///   identity, the member reads active and is not materialized, and
///   `meta/identity/resolved_tools` answers its last build from the record
///   (`tools_source` `recorded`);
/// - the governed input's wake prepares the member's build with exactly one
///   `customize_build`, whose inputs equal those of the member's last build
///   before the restart: the identity, every roster identity as
///   `active_peers`, the managed edges compared as a set, the runtime's
///   services, the roster row and the base draft. An in-process customizer
///   is handed no scope id: the publication of its build is the scope;
/// - that `customize_build` is answered before the host's bind: the bind is
///   handed the member's session and its pinned controller, after exactly
///   one build;
/// - the member is built once, on the session and generation it had, and
///   the restored process calls no `after_create`;
/// - `resolved_tools` then answers `live`, and the member's next turn calls
///   the tool through the handler that one `customize_build` registered,
///   never through one an earlier build registered.
///
/// The host's binder refuses the governed input, so nothing is submitted and
/// the tool call is the next ordinary turn's: native admission takes only a
/// bound input (an authority association and an ingress context) on a member
/// whose runtime machine carries the host's native grant work configuration,
/// and the one fixture that admits one (`support/governed_mob.rs`) is an
/// in-memory mob spawned live, with no roster, customizer, build preparer or
/// restart.
///
/// RED on Meta 0.9.5 behaviour (by reasoning; not run there): no
/// `customize_build` and no materialized member before the input (the
/// restart made one call per roster identity and built every member),
/// `tools_source` `recorded` and `live` (the field is new in 0.9.6), exactly
/// one `customize_build` for the governed input (its member was already
/// built, so the wake made none), and no `after_create` in the restored
/// process. The status, the session and generation, and the tool routing
/// would hold on both. RED on a builder whose identity bridge has no
/// controller pin source: the send is refused `Unsupported` after the wake,
/// and no bind is reached.
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_input_after_a_clean_restart_makes_one_customize_build_and_no_after_create() {
    const MOB_ID: &str = "customizer-tools-restart-q";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let served = Arc::new(std::sync::Mutex::new(Vec::new()));

    // The first boot seats both identities, wires them, and builds the
    // member with one ordinary delivery.
    let (first_status, first_tools, first_build) = {
        let model = ToolTurnModel::default();
        let customizer = Arc::new(RecordingCustomizer::new("first-boot", &served));
        let runtime = boot_governed(MOB_ID, &state, model.clone(), customizer.clone()).await;
        send_member(&runtime, "hello")
            .await
            .expect("send boot-1 turn");
        wait_until("boot 1's turn", || model.count() >= 1).await;
        let requests = model.requests();
        let request = requests.first().expect("boot 1's request");
        assert!(offers_tool(request), "the build offers the customizer tool");
        let answer = resolved_tools_answer(&runtime).await;
        assert_eq!(answer["tools_source"], "live", "{answer}");
        let tools = answered_tools(&answer);
        assert!(tools.contains(&TOOL.to_string()), "{answer}");
        let status = member_status(&runtime).await;
        // The seats' after_create shows the hook is called at all, so the
        // restored process's zero below is not vacuous.
        wait_until("the seats' after_create", || customizer.after_creates() > 0).await;
        let builds = customizer.builds();
        let build = builds
            .iter()
            .rfind(|call| call.inputs.identity == id(MEMBER))
            .expect("the member's build was prepared")
            .inputs
            .clone();
        assert!(
            build
                .managed_edges
                .contains(&edge_ends(&edge(MEMBER, GOVERNED_PEER))),
            "the member's build context carries its managed edge: {build:?}"
        );
        // The build's report is written off Core's actor once the member is
        // live.
        sleep(Duration::from_millis(500)).await;
        runtime.shutdown().await;
        (status, tools, build)
    };
    assert!(
        served.lock().unwrap().is_empty(),
        "the first boot calls no tool"
    );

    // The clean restart builds nothing and calls the host for no identity.
    let model = ToolTurnModel::default();
    let customizer = Arc::new(RecordingCustomizer::new("restored", &served));
    let runtime = boot_governed(MOB_ID, &state, model.clone(), customizer.clone()).await;
    assert!(
        customizer.builds().is_empty(),
        "the restart builds no member, so it calls customize_build for none: {:?}",
        customizer.builds()
    );
    let status = member_status(&runtime).await;
    assert_eq!(status.state, IdentityLifecycleState::Active, "{status:?}");
    assert!(status.customizer_tools_pending.is_none(), "{status:?}");
    assert_eq!(
        member_run_starts_held(&runtime).await,
        None,
        "no member is materialized"
    );
    let first_session = first_status
        .session_id
        .as_ref()
        .expect("the first boot's member has a session")
        .to_string();
    let answer = resolved_tools_answer(&runtime).await;
    assert_eq!(answer["tools_source"], "recorded", "{answer}");
    assert_eq!(answer["session_id"], first_session.as_str(), "{answer}");
    assert_eq!(answered_tools(&answer), first_tools, "{answer}");
    assert_eq!(model.count(), 0, "nothing ran");

    // The first governed input: Core wakes the member for it before its
    // bind, and the host's bind refuses it, so nothing is submitted.
    let binder = Arc::new(RefusingBinder::new(Arc::clone(&customizer)));
    let sent = runtime
        .identity_runtime()
        .expect("identity runtime")
        .send_governed_with_turn_ticket(
            &id(MEMBER),
            None,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(GOVERNED_INPUT, None)),
            binder.clone(),
        )
        .await;

    // Exactly one customize_build, made by the governed input's wake, with
    // the inputs of the member's last build before the restart.
    let builds = customizer.builds();
    let [woken] = builds.as_slice() else {
        panic!(
            "the governed input's wake prepares the member's build exactly once, inline: \
             {builds:?} ({sent:?})"
        );
    };
    assert_eq!(
        woken.inputs, first_build,
        "the wake's customize_build has the member's context, roster row and base draft"
    );

    // The bind comes after that one build, handed the member's session and
    // its pinned controller; it refuses, so nothing ran.
    assert!(
        matches!(
            &sent,
            Err(IdentityRuntimeError::GovernedInput(GovernedInputError::Refused(refusal)))
                if refusal.detail == BIND_REFUSED
        ),
        "the governed input reaches the host's bind: {sent:?}"
    );
    assert_eq!(binder.builds_at_bind(), [1], "{:?}", binder.binds());
    assert_eq!(
        binder.binds(),
        [HandedBind {
            session_id: first_status
                .session_id
                .clone()
                .expect("the first boot's member has a session"),
            controller: tool_turn_controller(),
            builds: 1,
        }],
        "the bind is handed the member's session and its pinned controller"
    );
    assert_eq!(model.count(), 0, "nothing was submitted, so nothing ran");

    // The member was built once, on the session and generation it had.
    assert_eq!(
        member_run_starts_held(&runtime).await,
        Some(false),
        "the governed input's wake built the member, unheld"
    );
    let status = member_status(&runtime).await;
    assert_eq!(status.session_id, first_status.session_id, "{status:?}");
    assert_eq!(status.generation, first_status.generation, "{status:?}");
    let answer = resolved_tools_answer(&runtime).await;
    assert_eq!(answer["tools_source"], "live", "{answer}");
    assert_eq!(answered_tools(&answer), first_tools, "{answer}");

    // The member's next turn calls the tool through the handler the wake's
    // customize_build registered.
    send_member(&runtime, TOOL_TURN)
        .await
        .expect("send the tool turn");
    wait_until("the tool call", || !served.lock().unwrap().is_empty()).await;
    wait_until("the turn after the tool call", || model.count() >= 2).await;
    let requests = model.requests();
    let request = requests.first().expect("the tool turn's request");
    assert!(offers_tool(request), "the turn offers the customizer tool");
    assert_eq!(
        *served.lock().unwrap(),
        std::slice::from_ref(&woken.tag),
        "the call reaches the handler of the wake's customize_build, never an earlier build's"
    );
    assert_eq!(
        customizer.builds().len(),
        1,
        "the live member's turn prepares no other build"
    );
    assert_eq!(
        customizer.after_creates(),
        0,
        "the restored process calls no after_create: a wake's build never calls it"
    );
    runtime.shutdown().await;
}

/// A runtime the library builder composes gives its identity bridge the
/// controller pin source its mob runtime recorded, as both gateways do, so a
/// governed input reaches the host's bind through both doors: each bind is
/// handed the member's session and the controller its session owner pinned.
/// RED on a builder whose identity bridge has no controller pin source: the
/// governed send and dispatch are refused `Unsupported` ("no controller pin
/// source is installed for this bridge") after Meta has woken and built the
/// member, and no bind is reached.
#[tokio::test(flavor = "multi_thread")]
async fn a_builder_runtime_pins_the_member_controller_so_governed_input_reaches_the_hosts_bind() {
    const MOB_ID: &str = "customizer-tools-restart-r";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let served = Arc::new(std::sync::Mutex::new(Vec::new()));
    let model = ToolTurnModel::default();
    let customizer = Arc::new(RecordingCustomizer::new("builder", &served));
    let runtime = boot_governed(MOB_ID, &state, model.clone(), customizer.clone()).await;
    let binder = Arc::new(RefusingBinder::new(Arc::clone(&customizer)));

    // The external door wakes the seated member for its bind; the trusted
    // host's internal door then reaches the woken member's bind too.
    let sent = runtime
        .identity_runtime()
        .expect("identity runtime")
        .send_governed_with_turn_ticket(
            &id(MEMBER),
            None,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(GOVERNED_INPUT, None)),
            binder.clone(),
        )
        .await;
    let dispatched = runtime
        .identity_runtime()
        .expect("identity runtime")
        .dispatch_governed_with_turn_ticket(
            &id(MEMBER),
            None,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(GOVERNED_INPUT, None)),
            binder.clone(),
        )
        .await;
    for (door, outcome) in [("send", &sent), ("dispatch", &dispatched)] {
        assert!(
            matches!(
                outcome,
                Err(IdentityRuntimeError::GovernedInput(GovernedInputError::Refused(refusal)))
                    if refusal.detail == BIND_REFUSED
            ),
            "the governed {door} reaches the host's bind: {outcome:?}"
        );
    }
    let session_id = member_status(&runtime)
        .await
        .session_id
        .expect("the member has a session");
    let handed = binder
        .binds()
        .into_iter()
        .map(|bind| (bind.session_id, bind.controller))
        .collect::<Vec<_>>();
    assert_eq!(
        handed,
        [
            (session_id.clone(), tool_turn_controller()),
            (session_id, tool_turn_controller()),
        ],
        "each bind is handed the member's session and the controller its session owner pinned"
    );
    assert_eq!(model.count(), 0, "both binds refused, so nothing ran");
    runtime.shutdown().await;
}

/// Set in the child process of
/// [`a_bootstrap_spec_declaring_its_customizer_continues_a_killed_run_only_with_its_tools`]:
/// the state directory its first boot runs in.
const DECLARED_CHILD_STATE: &str = "META_CUSTOMIZER_TOOLS_DECLARED_CHILD_STATE";

/// A mob over `state` composed the way a host composes one with
/// `UnifiedRuntime::bootstrap`: a persistent spec whose members are built on
/// `model` and which declares `customizer`
/// (`MobBootstrapSpec::with_agent_customizer`), bootstrapped and not yet
/// given an identity context.
async fn bootstrap_declaring(
    mob_id: &str,
    state: &Path,
    model: WorkerModel,
    customizer: Arc<dyn AgentCustomizer>,
) -> meerkat_meta::UnifiedRuntime {
    std::fs::create_dir_all(state).expect("state root");
    let (storage, provenance) =
        meerkat_meta::mob_composition_manifest::persistent_mob_storage(state.join("mob.sqlite3"))
            .expect("open persistent mob storage");
    let spec = meerkat_meta::MobBootstrapSpec::persistent(
        MobDefinition::from_toml(&MOB_TOML.replace("{MOB_ID}", mob_id)).expect("definition"),
        storage,
        provenance,
        state.to_path_buf(),
        16,
    )
    .await
    .expect("compose persistent Meta stores")
    .with_child_mob_storage(meerkat_meta::ChildMobStorage::ProcessBound)
    .with_options(meerkat_meta::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(model)),
    })
    .with_agent_customizer(customizer);
    meerkat_meta::UnifiedRuntime::bootstrap(
        spec,
        meerkat_meta::MetaConfig {
            modules: Vec::new(),
            discovery: meerkat_meta::DiscoverySpec {
                namespace: mob_id.to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Duration::from_secs(10),
    )
    .await
    .expect("bootstrap the UnifiedRuntime")
}

/// The identity context such a host installs over `runtime`: the state
/// directory's continuity substrate, the identity bridge every composition
/// builds (`gateway_wiring::identity_session_bridge`), a roster that lists
/// [`MEMBER`], and `customizer` as the context's own.
async fn bootstrap_identity_context(
    runtime: &meerkat_meta::UnifiedRuntime,
    mob_id: &str,
    state: &Path,
    customizer: Option<Arc<dyn AgentCustomizer>>,
) -> Arc<meerkat_meta::identity_first::IdentityFirstRuntimeContext> {
    let substrate =
        meerkat_meta::gateway_wiring::open_identity_substrate(&state.join("continuity.sqlite3"))
            .await
            .expect("open the identity substrate");
    let handle = meerkat_meta::gateway_wiring::identity_mob_handle(runtime);
    let bridge = meerkat_meta::gateway_wiring::identity_session_bridge(
        runtime.mob_runtime(),
        handle.clone(),
    );
    let identity_runtime = meerkat_meta::identity_first::IdentityRuntime::new(
        meerkat_meta::identity_first::IdentityRuntimeConfig {
            continuity_store: substrate.continuity_store,
            lease_provider: substrate.lease_provider,
            runtime_instance_id: mob_id.to_string(),
            has_runtime_store: true,
            durability_policy: meerkat_meta::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(bridge)),
            default_timeout: None,
        },
    )
    .with_runtime_services(meerkat_meta::identity_first::AgentRuntimeServices::new(
        handle,
    ));
    Arc::new(
        meerkat_meta::identity_first::IdentityFirstRuntimeContext::new(
            Arc::new(identity_runtime),
            Arc::new(ListedRoster(&[MEMBER])),
            None,
            customizer,
            Some(runtime.mob_handle().definition().clone()),
        ),
    )
}

/// [`bootstrap_declaring`] with its identity context installed and
/// bootstrapped. The context carries the declared customizer too.
async fn boot_declaring(
    mob_id: &str,
    state: &Path,
    model: WorkerModel,
    customizer: Arc<dyn AgentCustomizer>,
) -> meerkat_meta::UnifiedRuntime {
    let mut runtime = bootstrap_declaring(mob_id, state, model, Arc::clone(&customizer)).await;
    let context = bootstrap_identity_context(&runtime, mob_id, state, Some(customizer)).await;
    runtime
        .install_and_bootstrap_identity_first_context(context, &[durable_spec(MEMBER)])
        .await
        .expect("install and bootstrap the identity context");
    runtime
}

/// A host that composes with `UnifiedRuntime::bootstrap` and declares its
/// agent customizer on the spec (`MobBootstrapSpec::with_agent_customizer`)
/// gets the builder's guarantee across a kill. The kill leaves the member's
/// run open (no Stop is recorded, so the mob log stays Running), so the
/// restart's mob restores the member while `UnifiedRuntime::bootstrap`
/// builds it, before the host has composed its identity runtime, and the
/// wiring the declaration armed holds that build. The identity bootstrap's
/// roster pass publishes the member's tools with one `customize_build`, run
/// with the runtime's real context, and releases the hold. Only then does the
/// open run continue, and every request it makes offers the tool. RED with
/// the declaration's early arming dropped (the spec's wiring deferred to the
/// identity bootstrap): the start builds the member unprepared and unheld,
/// its run continues without the tool, and no `customize_build` runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_bootstrap_spec_declaring_its_customizer_continues_a_killed_run_only_with_its_tools() {
    const MOB_ID: &str = "customizer-tools-restart-s";
    if let Some(state) = std::env::var_os(DECLARED_CHILD_STATE) {
        // The child: the member's run commits a round and waits on its
        // model, then the process exits without shutdown.
        let model = WorkerModel::holding();
        let runtime = boot_declaring(
            MOB_ID,
            Path::new(&state),
            model.clone(),
            Arc::new(ToolCustomizer::default()),
        )
        .await;
        send_member(&runtime, &worker_turn(MEMBER))
            .await
            .expect("send the member its work");
        wait_for_round(&model, MEMBER).await;
        std::process::exit(0);
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "a_bootstrap_spec_declaring_its_customizer_continues_a_killed_run_only_with_its_tools",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(DECLARED_CHILD_STATE, &state)
        .status()
        .expect("run the first boot in a child process");
    assert!(child.success(), "the child's first boot failed: {child}");

    let model = WorkerModel::default();
    let customizer = Arc::new(RealContextCustomizer::default());
    let runtime = boot_declaring(MOB_ID, &state, model.clone(), customizer.clone()).await;
    wait_for_round(&model, MEMBER).await;
    let continued = model.after_round_requests(MEMBER);
    assert!(
        continued.iter().all(|request| offers_tool(request)),
        "the open run continued only with the customizer tool: {continued:?}"
    );
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        1,
        "the roster pass's publication, with the real context; the declared wiring held the \
         start's build without calling the host"
    );
    let status = member_status(&runtime).await;
    assert!(status.customizer_tools_pending.is_none(), "{status:?}");
    assert_eq!(member_run_starts_held(&runtime).await, Some(false));
    assert!(resolved_tools(&runtime).await.contains(&TOOL.to_string()));
    runtime.shutdown().await;
}

/// The door's own wake (`IdentityRuntime::materialize`) of a member whose
/// build preparation the host refuses retryably, through the real session
/// bridge and Core: Core records the build preparation retry and refuses
/// the wake for it (`BuildPreparationRetry`), the door retries the
/// preparation once, as it does for a delivery's wake, and the host is
/// asked twice. The second refusal answers with the customizer's own
/// failure, typed and retryable.
///
/// RED before the door's own wake read Core's refusal typed: the host was
/// asked once, and the materialization answered Core's rendering as mob
/// text.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_build_preparation_on_the_doors_own_wake_is_retried_once() {
    const MOB_ID: &str = "customizer-tools-restart-door-wake";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    first_boot(MOB_ID, &state).await;

    let capture = CaptureClient::default();
    let customizer = Arc::new(ToolCustomizer {
        calls: AtomicUsize::new(0),
        fail_calls: BTreeSet::from([1, 2]),
    });
    let runtime = boot(
        MOB_ID,
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        0,
        "the restart builds no member"
    );

    let refused = runtime
        .identity_runtime()
        .expect("identity runtime")
        .materialize(&id(MEMBER))
        .await;
    let calls = customizer.calls.load(Ordering::SeqCst);
    runtime.shutdown().await;
    assert_eq!(
        calls, 2,
        "the wake's preparation and the door's one retry of it: {refused:?}"
    );
    match &refused {
        Err(IdentityRuntimeError::IdentityWakeRetryable { detail, .. }) => {
            assert!(detail.contains("host refused build 2"), "{detail}");
        }
        other => {
            panic!("the materialization is refused retryable with the host's reason: {other:?}")
        }
    }
}

// ---------------------------------------------------------------------------
// Who manages a mob an identity created with its own `mob_create`, after the
// identity's next incarnations. Each question is the identity's own: its own
// turn makes the call through its own tools, and the row reads the tool's
// answer from the identity's transcript.
// ---------------------------------------------------------------------------

/// The host turn marker of these rows: a request whose transcript holds it
/// is the asking identity's.
const OWN_TOOLS_TURN: &str = "USE-YOUR-OWN-MOB-TOOLS";

/// A scripted model: on a request whose transcript holds [`OWN_TOOLS_TURN`]
/// it makes, once each and in order, the queued calls whose id the
/// transcript does not hold yet, and it records every tool result a
/// transcript holds.
#[derive(Clone, Default)]
struct OwnToolsModel {
    script: Arc<std::sync::Mutex<Vec<(String, &'static str, serde_json::Value)>>>,
    tool_results: Arc<std::sync::Mutex<BTreeMap<String, (bool, String)>>>,
}

impl OwnToolsModel {
    fn queue(&self, id: &str, tool: &'static str, args: serde_json::Value) {
        self.script
            .lock()
            .unwrap()
            .push((id.to_string(), tool, args));
    }

    /// The recorded result of the call `id`: whether it was an error, and
    /// its text.
    fn result(&self, id: &str) -> (bool, String) {
        self.tool_results
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| panic!("the call {id} has a recorded result"))
    }
}

impl meerkat_client::LlmClient for OwnToolsModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let transcript = serde_json::to_string(&request.messages).unwrap_or_default();
        {
            let mut recorded = self.tool_results.lock().unwrap();
            for message in &request.messages {
                if let meerkat_core::Message::ToolResults { results, .. } = message {
                    for result in results {
                        recorded.insert(
                            result.tool_use_id.clone(),
                            (result.is_error, result.text_content()),
                        );
                    }
                }
            }
        }
        let call = if transcript.contains(OWN_TOOLS_TURN) {
            self.script
                .lock()
                .unwrap()
                .iter()
                .find(|(id, _, _)| !transcript.contains(id.as_str()))
                .cloned()
        } else {
            None
        };
        let [usage, done] = llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            if call.is_some() {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
        );
        Box::pin(async_stream::stream! {
            match call {
                Some((id, name, args)) => {
                    yield Ok(LlmEvent::ToolCallComplete {
                        id,
                        name: name.to_string(),
                        args,
                        meta: None,
                    });
                }
                None => {
                    yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// The definition of these rows: two roles that mount the mob tools (so a
/// move between them keeps the tools), and a worker role.
const OWN_TOOLS_MOB_TOML: &str = r#"
[mob]
id = "{MOB_ID}"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
mob = true

[profiles.steward]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.steward.tools]
comms = true
mob = true

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
durable = true

[profiles.worker.tools]
comms = true
"#;

async fn boot_own_tools(
    mob_id: &str,
    state: &Path,
    model: OwnToolsModel,
) -> meerkat_meta::UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&OWN_TOOLS_MOB_TOML.replace("{MOB_ID}", mob_id))
                    .expect("definition"),
            )
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[CREATOR])))
            .identity_runtime_instance_id(mob_id)
            .comms(true)
            .default_llm_client(Arc::new(model))
            .build(),
    )
    .await
    .expect("build the runtime")
}

/// One turn of `who` that makes `calls` through its own tools, in order, and
/// answers each call's recorded result: whether it was an error, and its
/// text. `stage` makes the call ids unique.
async fn own_tools_turn(
    runtime: &meerkat_meta::UnifiedRuntime,
    model: &OwnToolsModel,
    who: &str,
    stage: &str,
    calls: Vec<(&'static str, serde_json::Value)>,
) -> Vec<(bool, String)> {
    let ids: Vec<String> = calls
        .iter()
        .enumerate()
        .map(|(n, (tool, _))| format!("own-{stage}-{n}-{tool}"))
        .collect();
    for (id, (tool, args)) in ids.iter().zip(calls) {
        model.queue(id, tool, args);
    }
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .send_awaiting_commit(
            &id(who),
            &meerkat_core::ContentInput::Text(format!("{OWN_TOOLS_TURN} {stage}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{who}'s turn at {stage} commits: {error}"));
    ids.iter().map(|id| model.result(id)).collect()
}

/// `who` creates the mob `child` with its own `mob_create` and seats
/// [`OWNED_WORKER`] in it with its own `mob_spawn_member`.
async fn creates_a_mob_with_its_own_tool(
    runtime: &meerkat_meta::UnifiedRuntime,
    model: &OwnToolsModel,
    who: &str,
    child: &str,
) {
    let answers = own_tools_turn(
        runtime,
        model,
        who,
        &format!("create-{child}"),
        vec![
            (
                "mob_create",
                serde_json::json!({ "definition": {
                    "id": child,
                    "profiles": { "worker": {
                        "model": "gpt-5.5",
                        "runtime_mode": "turn_driven",
                        "tools": { "comms": true }
                    } }
                } }),
            ),
            (
                "mob_spawn_member",
                serde_json::json!({
                    "mob_id": child, "profile": "worker", "member_id": OWNED_WORKER
                }),
            ),
        ],
    )
    .await;
    for (is_error, text) in &answers {
        assert!(
            !is_error,
            "{who} creates {child} and seats its worker: {text}"
        );
    }
    let provenance = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the agent mob tools' state")
        .handle_for(&meerkat_mob::MobId::from(child))
        .await
        .expect("the created mob is hosted")
        .provenance()
        .await
        .expect("the creation record reads");
    assert!(
        matches!(
            provenance,
            meerkat_mob::MobProvenance::OperatorMember { .. }
        ),
        "the record names {who} as the creator of its own create: {provenance:?}"
    );
}

/// `who` lists `child` and reads and checks its member, through its own
/// tools.
async fn assert_manages_its_mob(
    runtime: &meerkat_meta::UnifiedRuntime,
    model: &OwnToolsModel,
    who: &str,
    child: &str,
    when: &str,
) {
    let answers = own_tools_turn(
        runtime,
        model,
        who,
        &format!("reads-{when}-{child}"),
        vec![
            ("mob_list", serde_json::json!({})),
            ("mob_list_members", serde_json::json!({ "mob_id": child })),
            (
                "mob_check_member",
                serde_json::json!({ "mob_id": child, "member_id": OWNED_WORKER }),
            ),
        ],
    )
    .await;
    let [list, members, check] = answers.as_slice() else {
        panic!("three answers: {answers:?}");
    };
    assert!(
        !list.0 && list.1.contains(child),
        "{when}: {who} lists the mob it created: {list:?}"
    );
    assert!(
        !members.0 && members.1.contains(OWNED_WORKER),
        "{when}: {who} reads the members of the mob it created: {members:?}"
    );
    assert!(
        !check.0,
        "{when}: {who} checks a member of the mob it created: {check:?}"
    );
}

/// `who` destroys `child` with its own tool, and the host no longer manages
/// the mob.
async fn assert_destroys_its_mob(
    runtime: &meerkat_meta::UnifiedRuntime,
    model: &OwnToolsModel,
    who: &str,
    child: &str,
    when: &str,
) {
    let answers = own_tools_turn(
        runtime,
        model,
        who,
        &format!("destroys-{when}-{child}"),
        vec![("mob_destroy", serde_json::json!({ "mob_id": child }))],
    )
    .await;
    assert!(
        !answers[0].0,
        "{when}: {who} destroys the mob it created: {:?}",
        answers[0]
    );
    assert!(
        runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the agent mob tools' state")
            .handle_for(&meerkat_mob::MobId::from(child))
            .await
            .is_err(),
        "{when}: the destroyed mob is no longer hosted"
    );
}

/// `who` does not list `child`, each of its calls on the mob is refused, and
/// the host still manages the mob.
async fn assert_manages_nothing_of(
    runtime: &meerkat_meta::UnifiedRuntime,
    model: &OwnToolsModel,
    who: &str,
    child: &str,
    when: &str,
) {
    let answers = own_tools_turn(
        runtime,
        model,
        who,
        &format!("refused-{when}-{child}"),
        vec![
            ("mob_list", serde_json::json!({})),
            ("mob_list_members", serde_json::json!({ "mob_id": child })),
            (
                "mob_check_member",
                serde_json::json!({ "mob_id": child, "member_id": OWNED_WORKER }),
            ),
            ("mob_destroy", serde_json::json!({ "mob_id": child })),
        ],
    )
    .await;
    let (list, calls) = answers.split_first().expect("four answers");
    assert!(
        !list.0 && !list.1.contains(child),
        "{when}: {who} does not list the mob: {list:?}"
    );
    for (is_error, text) in calls {
        assert!(
            *is_error && text.contains("not allowed"),
            "{when}: {who}'s call on the mob is refused: {text}"
        );
    }
    assert!(
        runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the agent mob tools' state")
            .handle_for(&meerkat_mob::MobId::from(child))
            .await
            .is_ok(),
        "{when}: the refused calls left the mob in place"
    );
}

/// The session `who` runs on and the generation of its member.
async fn seat_of(
    runtime: &meerkat_meta::UnifiedRuntime,
    who: &str,
) -> (meerkat_core::types::SessionId, meerkat_mob::Generation) {
    let status = runtime
        .identity_runtime()
        .expect("identity runtime")
        .status(&id(who))
        .await
        .expect("the identity's status");
    (
        status.session_id.expect("the identity runs on a session"),
        status
            .generation
            .expect("the identity's member has a generation"),
    )
}

/// An identity that created a mob with its own tool keeps managing it after
/// its respawn: it lists the mob, reads and checks its members in the same
/// process and after a restart of the gateway, and then destroys it.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawned_identity_keeps_managing_the_mob_it_created_across_a_restart() {
    const MOB_ID: &str = "customizer-tools-restart-own-a";
    let child = format!("{MOB_ID}-child");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = OwnToolsModel::default();

    let before = {
        let runtime = boot_own_tools(MOB_ID, &state, model.clone()).await;
        creates_a_mob_with_its_own_tool(&runtime, &model, CREATOR, &child).await;
        assert_manages_its_mob(&runtime, &model, CREATOR, &child, "before the respawn").await;
        let before = seat_of(&runtime, CREATOR).await;

        runtime
            .identity_runtime()
            .expect("identity runtime")
            .respawn(&id(CREATOR))
            .await
            .expect("the respawn completes");
        let after = seat_of(&runtime, CREATOR).await;
        assert_eq!(
            after.0, before.0,
            "the respawned identity keeps its session"
        );
        assert!(after.1 > before.1, "at a later generation: {after:?}");
        assert_manages_its_mob(
            &runtime,
            &model,
            CREATOR,
            &child,
            "after the respawn, in the same process",
        )
        .await;
        runtime.shutdown().await;
        after
    };

    let runtime = boot_own_tools(MOB_ID, &state, model.clone()).await;
    assert_eq!(
        seat_of(&runtime, CREATOR).await,
        before,
        "the restart keeps the respawned identity's seat"
    );
    assert_manages_its_mob(&runtime, &model, CREATOR, &child, "after a restart").await;
    assert_destroys_its_mob(&runtime, &model, CREATOR, &child, "after a restart").await;
    runtime.shutdown().await;
}

/// An identity reset onto a fresh session, and one deleted and listed again,
/// manages nothing its earlier session created: the mob is not listed and
/// every call on it is refused.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_on_a_fresh_session_manages_nothing_its_earlier_session_created() {
    const MOB_ID: &str = "customizer-tools-restart-own-b";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = OwnToolsModel::default();
    let runtime = boot_own_tools(MOB_ID, &state, model.clone()).await;
    let identities = runtime.identity_runtime().expect("identity runtime");

    let first = format!("{MOB_ID}-before-reset");
    creates_a_mob_with_its_own_tool(&runtime, &model, CREATOR, &first).await;
    let before = seat_of(&runtime, CREATOR).await;
    identities
        .reset(&id(CREATOR))
        .await
        .expect("the reset completes");
    assert_manages_nothing_of(&runtime, &model, CREATOR, &first, "after a reset").await;
    let reset = seat_of(&runtime, CREATOR).await;
    assert_ne!(reset.0, before.0, "a reset takes a fresh session");

    let second = format!("{MOB_ID}-before-delete");
    creates_a_mob_with_its_own_tool(&runtime, &model, CREATOR, &second).await;
    assert_manages_its_mob(&runtime, &model, CREATOR, &second, "before the delete").await;
    identities
        .delete_identity(&id(CREATOR))
        .await
        .expect("the delete completes");
    for child in [&first, &second] {
        assert_manages_nothing_of(&runtime, &model, CREATOR, child, "after a delete").await;
    }
    let deleted = seat_of(&runtime, CREATOR).await;
    assert_ne!(deleted.0, reset.0, "a delete takes a fresh session");
    runtime.shutdown().await;
}

/// The identity these rows create at run time: its record is its row in the
/// mob's identity store.
const MOVED_AGENT: &str = "moved-agent";

fn moved_agent_spec(profile: &str) -> meerkat_mob::CreatedIdentitySpec {
    meerkat_mob::CreatedIdentitySpec::new(MOVED_AGENT, profile).with_display_name("Moved agent")
}

fn identity_operation(name: &str) -> meerkat_mob::IdentityOperation {
    meerkat_mob::IdentityOperation::new(
        name,
        meerkat_core::approval::decision::Sha256Digest::of(name.as_bytes()).as_str(),
    )
}

/// Change the created identity's row from `from` to `to` through Core's
/// store-owned update, as the host.
async fn update_moved_agent(
    runtime: &meerkat_meta::UnifiedRuntime,
    operation: &str,
    from: &meerkat_mob::CreatedIdentitySpec,
    to: meerkat_mob::CreatedIdentitySpec,
) {
    let updated = runtime
        .mob_handle()
        .update_store_owned_identity(
            meerkat_mob::StoreOwnedIdentityUpdate::new(
                identity_operation(operation),
                meerkat_mob::IdentityChangeRequesterInput::host("console"),
                to,
                from.spec_digest().expect("the row's digest"),
            ),
            meerkat_mob::IdentityChangeConsentInput::Policy(
                meerkat_mob::IdentityPolicyOutcome::Allow,
            ),
        )
        .await
        .map(|updated| updated.change);
    assert!(
        matches!(
            updated,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "Core updates the created identity ({operation}): {updated:?}"
    );
}

/// Wait (bounded) until Core holds the created identity's member on
/// `session` at a generation after `after`, and answer it.
async fn moved_agent_successor_after(
    runtime: &meerkat_meta::UnifiedRuntime,
    session: &meerkat_core::types::SessionId,
    after: meerkat_mob::Generation,
    what: &str,
) -> meerkat_mob::Generation {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let seated = runtime.mob_handle().member_of_session(session).await;
        if let Some((identity, generation)) = &seated
            && identity.as_str() == MOVED_AGENT
            && *generation > after
        {
            return *generation;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: a successor is seated on the identity's session: {seated:?}"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

/// An identity created at run time that created a mob with its own tool
/// keeps managing it after its row moves it to another role, and after its
/// row changes its runtime mode: each change seats its successor on its own
/// session, and the successor lists the mob, reads and checks its members
/// and at last destroys it.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_identity_moved_to_another_role_or_mode_keeps_managing_the_mob_it_created() {
    const MOB_ID: &str = "customizer-tools-restart-own-c";
    let child = format!("{MOB_ID}-child");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = OwnToolsModel::default();
    let runtime = boot_own_tools(MOB_ID, &state, model.clone()).await;

    let created = runtime
        .mob_handle()
        .create_store_owned_identities(
            meerkat_mob::StoreOwnedIdentityCreate::new(
                identity_operation("create-moved-agent"),
                meerkat_mob::IdentityChangeRequesterInput::host("console"),
                vec![meerkat_mob::CreatedIdentityRequest::new(moved_agent_spec(
                    "personal",
                ))],
            ),
            meerkat_mob::IdentityChangeConsentInput::Policy(
                meerkat_mob::IdentityPolicyOutcome::Allow,
            ),
        )
        .await
        .map(|created| created.change);
    assert!(
        matches!(
            created,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "Core creates the identity: {created:?}"
    );
    runtime
        .identity_first_context()
        .expect("identity-first context installed")
        .refresh_desired_topology()
        .await
        .expect("the roster pass registers the created identity");

    creates_a_mob_with_its_own_tool(&runtime, &model, MOVED_AGENT, &child).await;
    assert_manages_its_mob(
        &runtime,
        &model,
        MOVED_AGENT,
        &child,
        "before its row changes",
    )
    .await;
    let (session, created_at) = seat_of(&runtime, MOVED_AGENT).await;

    let moved = moved_agent_spec("steward");
    update_moved_agent(
        &runtime,
        "move-moved-agent",
        &moved_agent_spec("personal"),
        moved.clone(),
    )
    .await;
    let after_move =
        moved_agent_successor_after(&runtime, &session, created_at, "after the role move").await;
    assert_manages_its_mob(
        &runtime,
        &model,
        MOVED_AGENT,
        &child,
        "after a move to another role",
    )
    .await;
    assert_eq!(
        seat_of(&runtime, MOVED_AGENT).await.0,
        session,
        "the role move keeps the identity's session"
    );

    update_moved_agent(
        &runtime,
        "remode-moved-agent",
        &moved,
        moved
            .clone()
            .with_runtime_mode_override(meerkat_mob::MobRuntimeMode::AutonomousHost),
    )
    .await;
    moved_agent_successor_after(
        &runtime,
        &session,
        after_move,
        "after the runtime-mode change",
    )
    .await;
    // The member runs autonomously now and takes no tracked turn: its
    // session's own mob tools are asked directly, as its next build holds
    // them (it may create mobs and manages none).
    let listed = session_tools_call(&runtime, &session, "mob_list", serde_json::json!({}))
        .await
        .expect("mob_list");
    assert!(
        listed.to_string().contains(&child),
        "after a runtime-mode change the identity lists the mob it created: {listed}"
    );
    let members = session_tools_call(
        &runtime,
        &session,
        "mob_list_members",
        serde_json::json!({ "mob_id": child }),
    )
    .await
    .expect("after a runtime-mode change the identity reads the members of the mob it created");
    assert!(members.to_string().contains(OWNED_WORKER), "{members}");
    session_tools_call(
        &runtime,
        &session,
        "mob_destroy",
        serde_json::json!({ "mob_id": child }),
    )
    .await
    .expect("after a runtime-mode change the identity destroys the mob it created");
    assert!(
        runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the agent mob tools' state")
            .handle_for(&meerkat_mob::MobId::from(child.as_str()))
            .await
            .is_err(),
        "the destroyed mob is no longer hosted"
    );
    runtime.shutdown().await;
}

/// One call of the agent mob tools of `session`, built as the session's next
/// build holds them: it may create mobs and manages none.
async fn session_tools_call(
    runtime: &meerkat_meta::UnifiedRuntime,
    session: &meerkat_core::types::SessionId,
    tool: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let tools = meerkat_mob_mcp::AgentMobToolSurface::new(
        runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the agent mob tools' state"),
        None,
        meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
            .expect("generated create-only authority"),
        "gpt-5.5".to_string(),
        session.clone(),
        None,
        None,
        None,
    );
    let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
    let outcome = tools
        .dispatch(ToolCallView {
            id: "session-tools-call",
            name: tool,
            args: &raw,
        })
        .await?;
    Ok(serde_json::from_str(&outcome.result.text_content())
        .unwrap_or_else(|_| serde_json::json!(outcome.result.text_content())))
}

// ---------------------------------------------------------------------------
// A created agent's reach to a mob it created, across an update of itself
// ---------------------------------------------------------------------------

/// The agent the reach row creates at run time, of the profile that carries
/// the mob tools.
const KEEPER: &str = "keeper";
/// The member the keeper spawns into the mob it creates.
const KEEPER_WORKER: &str = "kept-worker";
/// The host turn marker of the reach row: a request whose transcript holds
/// it is the keeper's.
const REACH_TURN: &str = "REACH-TURN";

/// The scripted model of the reach row: on the keeper's requests it makes,
/// once each and in order, the queued calls whose id its transcript does
/// not hold yet, and it records every tool result a transcript holds.
#[derive(Clone, Default)]
struct ReachModel {
    script: Arc<std::sync::Mutex<Vec<(String, &'static str, serde_json::Value)>>>,
    tool_results: Arc<std::sync::Mutex<BTreeMap<String, (bool, String)>>>,
}

impl ReachModel {
    fn queue(&self, id: &str, tool: &'static str, args: serde_json::Value) {
        self.script
            .lock()
            .unwrap()
            .push((id.to_string(), tool, args));
    }

    /// The result the keeper's model received for the call `id`: whether it
    /// is an error, and its text.
    fn result(&self, id: &str) -> (bool, String) {
        self.tool_results
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| panic!("the keeper's call {id} was answered"))
    }
}

impl meerkat_client::LlmClient for ReachModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let transcript = serde_json::to_string(&request.messages).unwrap_or_default();
        {
            let mut recorded = self.tool_results.lock().unwrap();
            for message in &request.messages {
                if let meerkat_core::Message::ToolResults { results, .. } = message {
                    for result in results {
                        recorded.insert(
                            result.tool_use_id.clone(),
                            (result.is_error, result.text_content()),
                        );
                    }
                }
            }
        }
        let call = if transcript.contains(REACH_TURN) {
            self.script
                .lock()
                .unwrap()
                .iter()
                .find(|(id, _, _)| !transcript.contains(id.as_str()))
                .cloned()
        } else {
            None
        };
        let [usage, done] = llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            if call.is_some() {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
        );
        Box::pin(async_stream::stream! {
            match call {
                Some((id, name, args)) => {
                    yield Ok(LlmEvent::ToolCallComplete {
                        id,
                        name: name.to_string(),
                        args,
                        meta: None,
                    });
                }
                None => {
                    yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// What the keeper's own mob tools answer about the mob it created, after
/// one turn of its own that asks them.
struct Reach {
    /// `mob_list`.
    list: (bool, String),
    /// `mob_list_members` of the mob it created.
    members: (bool, String),
    /// The keeper's seat as Core reads it by the keeper's session: its
    /// identity and generation.
    seat: Option<(meerkat_mob::AgentIdentity, meerkat_mob::Generation)>,
}

impl Reach {
    /// Whether the keeper lists the mob `child` and reads its member.
    fn holds(&self, child: &str) -> bool {
        !self.list.0
            && self.list.1.contains(child)
            && !self.members.0
            && self.members.1.contains(KEEPER_WORKER)
    }
}

/// One turn of the keeper that makes the queued calls, in order.
async fn keeper_turn(runtime: &meerkat_meta::UnifiedRuntime, stage: &str) {
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .send_awaiting_commit(
            &id(KEEPER),
            &meerkat_core::ContentInput::Text(format!("{REACH_TURN} {stage}")),
        )
        .await
        .unwrap_or_else(|error| panic!("the keeper's turn ({stage}) commits: {error}"));
}

/// Ask the keeper, in a turn of its own, to list its mobs and the members
/// of `child`, and read its seat.
async fn keeper_reach(
    runtime: &meerkat_meta::UnifiedRuntime,
    model: &ReachModel,
    stage: &str,
    child: &str,
) -> Reach {
    let (list, members) = (
        format!("reach-{stage}-list"),
        format!("reach-{stage}-members"),
    );
    model.queue(&list, "mob_list", serde_json::json!({}));
    model.queue(
        &members,
        "mob_list_members",
        serde_json::json!({ "mob_id": child }),
    );
    keeper_turn(runtime, stage).await;
    let session = runtime
        .identity_runtime()
        .expect("identity runtime")
        .status(&id(KEEPER))
        .await
        .expect("the keeper's status")
        .session_id
        .expect("the keeper holds a session");
    let reach = Reach {
        list: model.result(&list),
        members: model.result(&members),
        seat: runtime.mob_handle().member_of_session(&session).await,
    };
    println!(
        "LU-REACH [{stage}] mob_list: is_error={} {}",
        reach.list.0, reach.list.1
    );
    println!(
        "LU-REACH [{stage}] mob_list_members: is_error={} {}",
        reach.members.0, reach.members.1
    );
    println!(
        "LU-REACH [{stage}] seat(identity, generation)={:?}",
        reach.seat
    );
    reach
}

/// Update the keeper's record from `before` to `after` through Core's
/// store-owned update, the call `meta/identity/update` and the agent tool
/// `identity_update` end in.
async fn update_keeper(
    runtime: &meerkat_meta::UnifiedRuntime,
    operation: &str,
    before: &meerkat_mob::CreatedIdentitySpec,
    after: &meerkat_mob::CreatedIdentitySpec,
) {
    let updated = runtime
        .mob_handle()
        .update_store_owned_identity(
            meerkat_mob::StoreOwnedIdentityUpdate::new(
                meerkat_mob::IdentityOperation::new(
                    operation,
                    meerkat_core::approval::decision::Sha256Digest::of(operation.as_bytes())
                        .as_str(),
                ),
                meerkat_mob::IdentityChangeRequesterInput::host("console"),
                after.clone(),
                before.spec_digest().expect("the record's digest"),
            ),
            meerkat_mob::IdentityChangeConsentInput::Policy(
                meerkat_mob::IdentityPolicyOutcome::Allow,
            ),
        )
        .await;
    println!(
        "LU-REACH update {operation}: {:?}",
        updated.as_ref().map(|updated| &updated.applied)
    );
    assert!(
        matches!(
            updated.as_ref().map(|updated| &updated.change),
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "Core commits the update {operation}: {updated:?}"
    );
}

/// A created agent that created a mob with its own tools still lists it,
/// reads its members and can destroy it after a label-only update of
/// itself. `keeper` is created at run time of the profile that carries the
/// mob tools; in a turn of its own it creates a mob and spawns a member
/// into it. A display-name-only update of the keeper is the control: its
/// tools answer as before. A label-only update then changes the keeper's
/// record, and the keeper's next turn lists the mob, reads its member and
/// destroys it: the update seated no successor of the keeper, so Core still
/// reads the keeper's session as the member that created the mob.
///
/// RED before a label change kept the seat: the label update seated a
/// same-session successor of the keeper (one generation up), and the mob's
/// creator check compares the member's generation exactly, so `mob_list`
/// answered no mob and `mob_list_members` and `mob_destroy` were refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_agent_still_manages_the_mob_it_created_after_a_label_only_update() {
    const MOB_ID: &str = "customizer-tools-restart-label-reach";
    let child = format!("{MOB_ID}-child");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = ReachModel::default();
    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&AGENT_SPAWN_MOB_TOML.replace("{MOB_ID}", MOB_ID))
                    .expect("definition"),
            )
            .persistent_state(&state)
            .continuity_from_state_dir(&state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[CREATOR])))
            .identity_runtime_instance_id(MOB_ID)
            .comms(true)
            .default_llm_client(Arc::new(model.clone()))
            .build(),
    )
    .await
    .expect("build the reach runtime");

    let created = meerkat_mob::CreatedIdentitySpec::new(KEEPER, "personal")
        .with_display_name("Keeper")
        .with_labels(BTreeMap::from([("team".to_string(), "alpha".to_string())]));
    let committed = runtime
        .mob_handle()
        .create_store_owned_identities(
            meerkat_mob::StoreOwnedIdentityCreate::new(
                meerkat_mob::IdentityOperation::new(
                    "create-keeper",
                    meerkat_core::approval::decision::Sha256Digest::of(b"create-keeper").as_str(),
                ),
                meerkat_mob::IdentityChangeRequesterInput::host("console"),
                vec![meerkat_mob::CreatedIdentityRequest::new(created.clone())],
            ),
            meerkat_mob::IdentityChangeConsentInput::Policy(
                meerkat_mob::IdentityPolicyOutcome::Allow,
            ),
        )
        .await
        .map(|created| created.change);
    assert!(
        matches!(
            committed,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "Core creates the keeper: {committed:?}"
    );
    runtime
        .identity_first_context()
        .expect("identity-first context installed")
        .refresh_desired_topology()
        .await
        .expect("the roster pass registers the keeper");

    // The keeper creates a mob and spawns a member into it, by its own tools.
    model.queue(
        "reach-create",
        "mob_create",
        serde_json::json!({ "definition": {
            "id": child,
            "profiles": { "worker": {
                "model": "gpt-5.5",
                "runtime_mode": "turn_driven",
                "tools": { "comms": true }
            } }
        } }),
    );
    model.queue(
        "reach-spawn",
        "mob_spawn_member",
        serde_json::json!({ "mob_id": child, "profile": "worker", "member_id": KEEPER_WORKER }),
    );
    let at_first = keeper_reach(&runtime, &model, "0-created", &child).await;
    for call in ["reach-create", "reach-spawn"] {
        let (failed, text) = model.result(call);
        assert!(!failed, "the keeper's {call} is answered: {text}");
    }
    assert!(
        at_first.holds(&child),
        "the keeper lists the mob it created and reads its member: mob_list {:?}, \
         mob_list_members {:?}",
        at_first.list,
        at_first.members
    );
    assert!(
        at_first.seat.is_some(),
        "the keeper is a member by its session"
    );

    // The control: a display-name-only update.
    let renamed = created.clone().with_display_name("Keeper of the desk");
    update_keeper(&runtime, "rename-keeper", &created, &renamed).await;
    let after_rename = keeper_reach(&runtime, &model, "1-renamed", &child).await;
    assert!(
        after_rename.holds(&child),
        "control: after a display-name-only update the keeper lists its mob and reads its \
         member: mob_list {:?}, mob_list_members {:?}",
        after_rename.list,
        after_rename.members
    );
    assert_eq!(
        after_rename.seat, at_first.seat,
        "control: a display-name-only update keeps the keeper's seat"
    );

    // The label-only update.
    let relabelled = renamed
        .clone()
        .with_labels(BTreeMap::from([("team".to_string(), "beta".to_string())]));
    update_keeper(&runtime, "relabel-keeper", &renamed, &relabelled).await;
    let labels = runtime
        .mob_handle()
        .get_member(&meerkat_mob::AgentIdentity::from(KEEPER))
        .await
        .expect("roster read")
        .expect("the keeper is a member")
        .labels;
    assert_eq!(
        labels.get("team").map(String::as_str),
        Some("beta"),
        "the roster reads the keeper's new label at once"
    );
    let after_relabel = keeper_reach(&runtime, &model, "2-relabelled", &child).await;
    // Then, in a turn of its own, the keeper destroys the mob it created.
    model.queue(
        "reach-destroy",
        "mob_destroy",
        serde_json::json!({ "mob_id": child }),
    );
    keeper_turn(&runtime, "3-destroy").await;
    let destroyed = model.result("reach-destroy");
    println!(
        "LU-REACH [2-relabelled] mob_destroy: is_error={} {}",
        destroyed.0, destroyed.1
    );
    runtime.shutdown().await;
    assert!(
        after_relabel.holds(&child),
        "after a label-only update the keeper still lists the mob it created and reads its \
         member: mob_list {:?} (before the update: {:?}), mob_list_members {:?} (before the \
         update: {:?}), seat {:?} (before the update: {:?})",
        after_relabel.list,
        after_rename.list,
        after_relabel.members,
        after_rename.members,
        after_relabel.seat,
        after_rename.seat
    );
    assert!(
        !destroyed.0,
        "after a label-only update the keeper can destroy the mob it created: {}",
        destroyed.1
    );
    assert_eq!(
        after_relabel.seat, after_rename.seat,
        "a label-only update keeps the keeper's seat: the same member at the same generation"
    );
}

// ---------------------------------------------------------------------------
// What a created agent's next build is composed with after a label update
// ---------------------------------------------------------------------------

/// A host customizer that records the labels each `customize_build` of the
/// keeper is handed: the registered row's (`spec.labels`) and the build
/// draft's (`draft.labels`).
#[derive(Default)]
struct LabelRecordingCustomizer {
    builds: std::sync::Mutex<Vec<(BTreeMap<String, String>, BTreeMap<String, String>)>>,
}

impl LabelRecordingCustomizer {
    fn builds(&self) -> Vec<(BTreeMap<String, String>, BTreeMap<String, String>)> {
        self.builds.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentCustomizer for LabelRecordingCustomizer {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        if context.identity == id(KEEPER) {
            self.builds
                .lock()
                .unwrap()
                .push((spec.labels.clone(), draft.labels.clone()));
        }
        Ok(())
    }
}

/// The next build of a created agent after a label-only update is composed
/// with its new labels, for a label added and for a label removed, and the
/// update seats nothing. `keeper` is created at run time and runs a turn,
/// so it is live and idle. A label-only update that adds a label releases
/// it; its next turn builds it again, and the host's customizer is handed
/// the row and the draft with the added label. A label-only update that
/// removes a label does the same, and that build's row and draft no longer
/// carry it: a host that composes an agent by its labels takes a label
/// away with the agent's next build, not at some later seat. Core reads
/// the keeper's session as the same member at the same generation
/// throughout.
///
/// A guard: it passes before a label change kept the seat too (the update
/// then seated a successor, whose first build was composed the same way).
/// It fails if the update leaves an idle live agent running on its old
/// build.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_agents_next_build_after_a_label_only_update_is_composed_with_its_new_labels() {
    const MOB_ID: &str = "customizer-tools-restart-label-build";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let model = ReachModel::default();
    let customizer = Arc::new(LabelRecordingCustomizer::default());
    let runtime = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(
                MobDefinition::from_toml(&AGENT_SPAWN_MOB_TOML.replace("{MOB_ID}", MOB_ID))
                    .expect("definition"),
            )
            .persistent_state(&state)
            .continuity_from_state_dir(&state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(ListedRoster(&[CREATOR])))
            .identity_runtime_instance_id(MOB_ID)
            .agent_customizer(customizer.clone())
            .comms(true)
            .default_llm_client(Arc::new(model.clone()))
            .build(),
    )
    .await
    .expect("build the runtime");

    let labels = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    };
    let created = meerkat_mob::CreatedIdentitySpec::new(KEEPER, "personal")
        .with_labels(labels(&[("team", "alpha"), ("tier", "bronze")]));
    let committed = runtime
        .mob_handle()
        .create_store_owned_identities(
            meerkat_mob::StoreOwnedIdentityCreate::new(
                meerkat_mob::IdentityOperation::new(
                    "create-keeper",
                    meerkat_core::approval::decision::Sha256Digest::of(b"create-keeper").as_str(),
                ),
                meerkat_mob::IdentityChangeRequesterInput::host("console"),
                vec![meerkat_mob::CreatedIdentityRequest::new(created.clone())],
            ),
            meerkat_mob::IdentityChangeConsentInput::Policy(
                meerkat_mob::IdentityPolicyOutcome::Allow,
            ),
        )
        .await
        .map(|created| created.change);
    assert!(
        matches!(
            committed,
            Ok(meerkat_mob::IdentityChangeOutcome::Committed { .. })
        ),
        "Core creates the keeper: {committed:?}"
    );
    runtime
        .identity_first_context()
        .expect("identity-first context installed")
        .refresh_desired_topology()
        .await
        .expect("the roster pass registers the keeper");
    let seat = || async {
        let session = runtime
            .identity_runtime()
            .expect("identity runtime")
            .status(&id(KEEPER))
            .await
            .expect("the keeper's status")
            .session_id
            .expect("the keeper holds a session");
        runtime.mob_handle().member_of_session(&session).await
    };
    // Whether `labels` carries every label of `row` that the host gave it.
    let carries = |labels: &BTreeMap<String, String>, row: &BTreeMap<String, String>| {
        row.iter()
            .all(|(key, value)| labels.get(key) == Some(value))
    };

    keeper_turn(&runtime, "0-first").await;
    let first = customizer.builds();
    let built = first.last().expect("the first turn builds the keeper");
    assert!(
        carries(&built.0, &created.labels) && carries(&built.1, &created.labels),
        "the first build is composed with the labels of the create: {first:?}"
    );
    let seated = seat().await;
    assert!(seated.is_some(), "the keeper is a member by its session");

    // A label is added.
    let added = created.clone().with_labels(labels(&[
        ("team", "alpha"),
        ("tier", "bronze"),
        ("zone", "north"),
    ]));
    update_keeper(&runtime, "add-zone", &created, &added).await;
    keeper_turn(&runtime, "1-added").await;
    let builds = customizer.builds();
    assert!(
        builds.len() > first.len(),
        "the keeper's next turn after the update builds it again: {builds:?}"
    );
    let built = builds.last().expect("a build");
    assert!(
        carries(&built.0, &added.labels) && carries(&built.1, &added.labels),
        "the next build is composed with the added label, in the row and in the draft: {built:?}"
    );
    let after_added = builds.len();
    assert_eq!(seat().await, seated, "the update seated nothing");

    // A label is removed.
    let removed = added
        .clone()
        .with_labels(labels(&[("team", "alpha"), ("zone", "north")]));
    update_keeper(&runtime, "remove-tier", &added, &removed).await;
    keeper_turn(&runtime, "2-removed").await;
    let builds = customizer.builds();
    runtime.shutdown().await;
    assert!(
        builds.len() > after_added,
        "the keeper's next turn after the update builds it again: {builds:?}"
    );
    let built = builds.last().expect("a build");
    assert!(
        carries(&built.0, &removed.labels) && carries(&built.1, &removed.labels),
        "the next build keeps the labels the row keeps: {built:?}"
    );
    assert!(
        !built.0.contains_key("tier") && !built.1.contains_key("tier"),
        "the removed label is gone from the row and from the draft the host's customizer is \
         handed at the next build: {built:?}"
    );
}
