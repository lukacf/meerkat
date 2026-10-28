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
//! - A member Core builds before the composition (a start that restores
//!   every member, or one whose run a kill left open) is held
//!   (`HostRunStartHoldReason::ToolsNotPublished`) until the roster pass
//!   publishes its tools and releases the hold: no run starts without them.
//!   A publication that fails keeps the member held, and work Core admits
//!   for it waits until a delivery publishes them.
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
use meerkat_meta::identity_first::contracts::{AgentCustomizer, RosterProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, CustomizerError,
    DurableAgentSpec, IdentityBootstrapMode, IdentityLifecycleState, IdentityRuntimeError,
    IdentityStatus, LocalExternalToolOverlay, MemberRestore, RosterContext, RosterError,
};
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
/// preparation of its first delivery's wake fails: the host refuses the
/// build. Core refuses the wake retryable and keeps the member Dormant, so
/// the delivery is refused, typed, with the host's reason, nothing is built
/// and nothing runs, and the identity's customizer tools read pending with
/// that reason. The next delivery wakes the member again: its build is
/// prepared with the tools, its turn offers them, and nothing stays pending.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_build_preparation_keeps_the_member_dormant_until_the_next_delivery_builds_it() {
    const MOB_ID: &str = "customizer-tools-restart-b";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    first_boot(MOB_ID, &state).await;

    let capture = CaptureClient::default();
    let customizer = Arc::new(ToolCustomizer {
        calls: AtomicUsize::new(0),
        fail_calls: BTreeSet::from([1]),
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
            assert!(detail.contains("host refused build 1"), "{detail}");
        }
        other => panic!("the delivery is refused retryable with the host's reason: {other:?}"),
    }
    let status = member_status(&runtime).await;
    let pending = status
        .customizer_tools_pending
        .expect("the refused preparation is recorded");
    assert!(
        pending.reason.contains("customize_build failed")
            && pending.reason.contains("host refused build 1"),
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
        .expect("the next delivery builds the member");
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

    // Work Core admits for the held member waits behind the hold.
    let member = runtime
        .mob_handle()
        .member(&meerkat_meta::member_comms_id::mob_member_id(MEMBER))
        .await
        .expect("Core holds the member");
    tokio::time::timeout(
        Duration::from_secs(10),
        member.send(
            meerkat_core::ContentInput::Text(QUEUED_WORK.to_string()),
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
/// each durable worker's run from its `run_resumed` to its `run_completed`,
/// and no event twice. The forwarder subscribes a member when the machine
/// state change of its wake makes it live (a pass that starts watching the
/// mob is followed by another at once), and replays the woken actor's
/// events from its first, so nothing between the boot wake and the attach
/// is missing; a member Core still holds unmaterialized (the roster's
/// identity, which owes no work) is asked nothing meanwhile.
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
            .default_llm_client(Arc::new(WorkerModel::default()))
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
        assert!(
            Instant::now() < deadline,
            "every resumed run reaches the event log: {:?}",
            WORKERS
                .iter()
                .map(|worker| (worker, log.agent_events_of(worker)))
                .collect::<Vec<_>>()
        );
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
