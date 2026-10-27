//! `customize_build` tools across a restart (#563), on the gateway-shaped
//! harness of `identity_first_head_canonical_resume` (state-dir continuity
//! substrate, persistent state, recording LLM double).
//!
//! - After a restart the restored member's tool scope already lists its
//!   customizer tools BEFORE any turn: Meta publishes them before the mob is
//!   lifted, so meerkat's restore builds the member with them.
//! - The first turn after the restart (the shape of a kickoff or a queued
//!   input admitted at activation) carries them in its LLM request.
//! - A pre-activation `customize_build` that fails is recorded as
//!   `IdentityStatus::customizer_tools_pending` with the typed reason, and the
//!   materialization that publishes clears it. The mob build also asks meerkat
//!   to hold the member's run starts (`HostRunStartHoldReason::ToolsNotPublished`)
//!   until then; here the publication releases that hold before the member's
//!   registration, so the member runs unheld once materialized. (The hold
//!   takes effect when meerkat's own resume revives the member first, a mob
//!   log left Running; the release itself is unit-tested in the identity
//!   runtime.)
//! - A first boot (a freshly created mob, so no lift either) owes no
//!   publication before its member exists: the customizer is called no extra
//!   time, and the early failure stays pending until the materialization
//!   publishes.
//! - A member meerkat's resume revives whose identity the roster no longer
//!   lists starts no run before the roster pass retires it: nothing
//!   publishes its customizer tools, and the mob build holds its run starts.
//!   A host-adopted identity's member, which the roster pass never retires
//!   and no roster publication covers, is not held: it runs as before.
//! - A host-adopted member that a process restart revives after a crash (the
//!   first process ended without shutdown, holding its identity lease) is
//!   converged again promptly, and work submitted to it runs.
//! - A durable member spawned at runtime, which no roster lists, is neither
//!   held nor retired by the roster pass after a kill or a graceful
//!   shutdown: its open run continues from its committed round, while an
//!   identity the roster really dropped is still retired.
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
    DurableAgentSpec, IdentityBootstrapMode, LocalExternalToolOverlay, RosterContext, RosterError,
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

/// A host customizer that declares [`TOOL`] only when its build context
/// carries the mob handle, as a host whose tools act on the mob does.
#[derive(Default)]
struct HandleToolCustomizer {
    calls: AtomicUsize,
}

#[async_trait]
impl AgentCustomizer for HandleToolCustomizer {
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
            .roster_provider(Arc::new(ListedRoster(roster)))
            .agent_customizer(customizer)
            .identity_bootstrap_mode(mode)
            .identity_runtime_instance_id(mob_id)
            .comms(true)
            .default_llm_client(Arc::new(capture))
            .build(),
    )
    .await
    .expect("build the gateway-shaped UnifiedRuntime")
}

async fn resolved_tools(runtime: &meerkat_meta::UnifiedRuntime) -> Vec<String> {
    let identity_runtime = runtime.identity_runtime().expect("identity runtime");
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    let session_id = status.session_id.expect("member has a session");
    meerkat_meta::mob_handle_runtime::resolved_tools_for_session(
        runtime.mob_runtime().session_service(),
        MEMBER,
        session_id,
    )
    .await
    .expect("resolved tools")
    .tools
}

/// The visible names of the member's tool scope, as its last model-call
/// boundary left them.
async fn scope_visible_tools(runtime: &meerkat_meta::UnifiedRuntime) -> Vec<String> {
    let identity_runtime = runtime.identity_runtime().expect("identity runtime");
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    let session_id = status.session_id.expect("member has a session");
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
/// (`None` while the member's session is not registered there).
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

/// Boot once (fresh materialization, tools present at spawn) and stop.
async fn first_boot(mob_id: &str, state: &Path) {
    let capture = CaptureClient::default();
    let runtime = boot(
        mob_id,
        state,
        capture.clone(),
        Arc::new(ToolCustomizer::default()),
        IdentityBootstrapMode::EagerMaterialize,
    )
    .await;
    assert!(resolved_tools(&runtime).await.contains(&TOOL.to_string()));
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .send(
            &id(MEMBER),
            &meerkat_core::ContentInput::Text("hello".to_string()),
        )
        .await
        .expect("send boot-1 turn");
    wait_for_turn(&capture, 1, "boot 1's turn").await;
    sleep(Duration::from_millis(500)).await;
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restored_member_lists_its_customizer_tools_before_the_first_turn() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    first_boot("customizer-tools-restart-a", &state).await;

    let capture = CaptureClient::default();
    let customizer = Arc::new(ToolCustomizer::default());
    let runtime = boot(
        "customizer-tools-restart-a",
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::EagerMaterialize,
    )
    .await;
    // Published before the lift (once) and again by the materialization.
    assert!(customizer.calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(capture.count(), 0, "no turn has run yet");
    assert!(
        resolved_tools(&runtime).await.contains(&TOOL.to_string()),
        "the restored member's tool scope lists its customizer tools before any turn"
    );
    let status = runtime
        .identity_runtime()
        .expect("identity runtime")
        .status(&id(MEMBER))
        .await
        .expect("status");
    assert!(status.customizer_tools_pending.is_none());

    // The first run after the restart, the shape of a kickoff admitted at
    // activation, offers the tool to the model.
    runtime
        .identity_runtime()
        .expect("identity runtime")
        .send(
            &id(MEMBER),
            &meerkat_core::ContentInput::Text("again".to_string()),
        )
        .await
        .expect("send boot-2 turn");
    wait_for_turn(&capture, 1, "boot 2's first turn").await;
    let request = capture.last().expect("captured request");
    assert!(
        request.contains(TOOL),
        "the first turn after the restart must offer the customizer tool"
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_early_customize_is_pending_until_the_materialization_publishes() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    first_boot("customizer-tools-restart-b", &state).await;

    // Both publications before the lift fail (the mob build's early one and
    // the bootstrap's), so the pending state is observable after the restore;
    // so does the first send's.
    let capture = CaptureClient::default();
    let customizer = Arc::new(ToolCustomizer {
        calls: AtomicUsize::new(0),
        fail_calls: BTreeSet::from([1, 2, 3]),
    });
    let runtime = boot(
        "customizer-tools-restart-b",
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    let identity_runtime = runtime.identity_runtime().expect("identity runtime");
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    let pending = status
        .customizer_tools_pending
        .expect("the failed early publication is recorded");
    assert!(
        pending
            .reason
            .contains("pre-activation customize_build failed")
            && pending.reason.contains("host refused build 2"),
        "{pending:?}"
    );
    // Core restores the member live at activation (it ran at the last stop),
    // and it registers with its run starts held until its tools are
    // published.
    assert_eq!(member_run_starts_held(&runtime).await, Some(true));

    // A send whose publication fails (the third build) is refused as
    // retryable: nothing is queued behind the hold.
    let refused = identity_runtime
        .send(
            &id(MEMBER),
            &meerkat_core::ContentInput::Text("refused".to_string()),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(meerkat_meta::identity_first::IdentityRuntimeError::IdentityWakeRetryable { .. })
        ),
        "{refused:?}"
    );
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    let pending = status
        .customizer_tools_pending
        .expect("the failed publication stays recorded");
    assert!(
        pending.reason.contains("host refused build 3"),
        "{pending:?}"
    );
    assert_eq!(member_run_starts_held(&runtime).await, Some(true));
    assert_eq!(capture.count(), 0, "nothing ran behind the hold");

    // The next send's delivery publishes them (the fourth build), which
    // clears the pending state and releases the hold.
    identity_runtime
        .send(
            &id(MEMBER),
            &meerkat_core::ContentInput::Text("wake".to_string()),
        )
        .await
        .expect("send materializes");
    wait_for_turn(&capture, 1, "the materializing turn").await;
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    assert!(status.customizer_tools_pending.is_none(), "{status:?}");
    assert_eq!(
        member_run_starts_held(&runtime).await,
        Some(false),
        "the released hold never reached the member's registration"
    );
    assert!(resolved_tools(&runtime).await.contains(&TOOL.to_string()));
    assert!(capture.last().expect("captured request").contains(TOOL));
    runtime.shutdown().await;
}

/// The mob build's early publication fails, so meerkat stages a run-start
/// hold for the restored member, and the bootstrap's own publication before
/// the lift succeeds. The hold is released once the mob is live: the member
/// runs with its tools, and nothing stays pending.
#[tokio::test(flavor = "multi_thread")]
async fn a_publication_before_the_lift_releases_the_hold_the_mob_build_staged() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    first_boot("customizer-tools-restart-c", &state).await;

    let capture = CaptureClient::default();
    let customizer = Arc::new(ToolCustomizer {
        calls: AtomicUsize::new(0),
        fail_calls: BTreeSet::from([1]),
    });
    let runtime = boot(
        "customizer-tools-restart-c",
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        2,
        "the mob build's early publication and the bootstrap's"
    );
    let identity_runtime = runtime.identity_runtime().expect("identity runtime");
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    assert!(status.customizer_tools_pending.is_none(), "{status:?}");
    assert_eq!(
        member_run_starts_held(&runtime).await,
        Some(false),
        "the hold the mob build staged is released once the mob is live"
    );
    assert!(resolved_tools(&runtime).await.contains(&TOOL.to_string()));

    identity_runtime
        .send(
            &id(MEMBER),
            &meerkat_core::ContentInput::Text("after the restart".to_string()),
        )
        .await
        .expect("send boot-2 turn");
    wait_for_turn(&capture, 1, "boot 2's first turn").await;
    assert!(capture.last().expect("captured request").contains(TOOL));
    runtime.shutdown().await;
}

/// Set in the child process of
/// [`a_member_restored_after_an_unclean_stop_lists_its_customizer_tools_before_the_first_turn`]:
/// the state directory its first boot runs in.
const UNCLEAN_STOP_CHILD_STATE: &str = "META_CUSTOMIZER_TOOLS_UNCLEAN_STOP_CHILD_STATE";

/// After an unclean stop (a kill: no Stop is recorded, so the mob log stays
/// Running) meerkat's own resume revives the member while the mob is built,
/// and no lift is staged. The mob build's early publication runs before the
/// mob handle exists, so a customizer whose tools need it fails there and the
/// member is held; the bootstrap must still publish its tools, with the
/// runtime's services (which carry the handle), and release the hold, as it
/// does after a clean stop.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_restored_after_an_unclean_stop_lists_its_customizer_tools_before_the_first_turn()
{
    const MOB_ID: &str = "customizer-tools-restart-d";
    if let Some(state) = std::env::var_os(UNCLEAN_STOP_CHILD_STATE) {
        // The child: the first boot, a turn, then an exit without shutdown.
        let capture = CaptureClient::default();
        let runtime = boot(
            MOB_ID,
            Path::new(&state),
            capture.clone(),
            Arc::new(ToolCustomizer::default()),
            IdentityBootstrapMode::EagerMaterialize,
        )
        .await;
        runtime
            .identity_runtime()
            .expect("identity runtime")
            .send(
                &id(MEMBER),
                &meerkat_core::ContentInput::Text("hello".to_string()),
            )
            .await
            .expect("send boot-1 turn");
        wait_for_turn(&capture, 1, "boot 1's turn").await;
        sleep(Duration::from_millis(500)).await;
        std::process::exit(0);
    }

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "a_member_restored_after_an_unclean_stop_lists_its_customizer_tools_before_the_first_turn",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(UNCLEAN_STOP_CHILD_STATE, &state)
        .status()
        .expect("run the first boot in a child process");
    assert!(child.success(), "the child's first boot failed: {child}");

    let capture = CaptureClient::default();
    let customizer = Arc::new(HandleToolCustomizer::default());
    let runtime = boot(
        MOB_ID,
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    let identity_runtime = runtime.identity_runtime().expect("identity runtime");
    // Before any turn: the tools are published and the hold is released, so
    // a run Core starts on its own (a restored queued input, a keep-alive
    // turn, a peer message) is not held and carries them.
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    assert!(
        status.customizer_tools_pending.is_none(),
        "the restored member's customizer tools are published before any turn: {status:?}"
    );
    assert_eq!(
        member_run_starts_held(&runtime).await,
        Some(false),
        "the hold the mob build staged is released after the unclean stop too"
    );
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        2,
        "the mob build's early publication (no handle yet) and the bootstrap's"
    );
    assert_eq!(capture.count(), 0, "no turn has run yet");
    // Meerkat's resume built the member before the publication, so its tool
    // scope still holds the base set it was built with; the resolved tools
    // are those its next model request carries, which include the published
    // tools.
    assert!(
        resolved_tools(&runtime).await.contains(&TOOL.to_string()),
        "the restored member's resolved tools list its customizer tools before any turn"
    );

    identity_runtime
        .send(
            &id(MEMBER),
            &meerkat_core::ContentInput::Text("after the unclean stop".to_string()),
        )
        .await
        .expect("send boot-2 turn");
    wait_for_turn(&capture, 1, "boot 2's first turn").await;
    assert!(capture.last().expect("captured request").contains(TOOL));
    assert!(resolved_tools(&runtime).await.contains(&TOOL.to_string()));
    // Once the member has run, its tool scope holds the same tools.
    assert_eq!(
        resolved_tools(&runtime).await,
        scope_visible_tools(&runtime).await
    );
    runtime.shutdown().await;
}

/// A first boot creates the mob, so no lift is staged: the bootstrap takes
/// the same no-lift path as a resume that found the mob Running. The mob
/// build's early publication still fails for a customizer whose tools need
/// the mob handle, so the identity is pending. Core holds no binding for it
/// yet, so the bootstrap owes no publication and calls the customizer no
/// extra time: the member does not exist until its materialization, which
/// publishes its tools itself, and the early failure stays pending until
/// then.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_boot_owes_no_customizer_publication_before_its_member_exists() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let capture = CaptureClient::default();
    let customizer = Arc::new(HandleToolCustomizer::default());
    let runtime = boot(
        "customizer-tools-restart-e",
        &state,
        capture.clone(),
        customizer.clone(),
        IdentityBootstrapMode::LazyMaterialize,
    )
    .await;
    let identity_runtime = runtime.identity_runtime().expect("identity runtime");
    assert_eq!(
        customizer.calls.load(Ordering::SeqCst),
        1,
        "only the mob build's early publication (no handle yet); a first boot owes none"
    );
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    let pending = status
        .customizer_tools_pending
        .expect("the early failure stays pending until the materialization publishes");
    assert!(
        pending
            .reason
            .contains("pre-activation customize_build failed")
            && pending.reason.contains("mob handle missing"),
        "{pending:?}"
    );
    assert_eq!(
        member_run_starts_held(&runtime).await,
        None,
        "a lazy first boot registers no member before its first send"
    );
    assert_eq!(capture.count(), 0, "no turn has run yet");

    // The first send materializes the member, and that materialization
    // publishes its tools with the runtime's services.
    identity_runtime
        .send(
            &id(MEMBER),
            &meerkat_core::ContentInput::Text("first".to_string()),
        )
        .await
        .expect("send materializes");
    wait_for_turn(&capture, 1, "the materializing turn").await;
    assert!(capture.last().expect("captured request").contains(TOOL));
    let status = identity_runtime.status(&id(MEMBER)).await.expect("status");
    assert!(status.customizer_tools_pending.is_none(), "{status:?}");
    assert!(resolved_tools(&runtime).await.contains(&TOOL.to_string()));
    runtime.shutdown().await;
}

/// Set in the child process of
/// [`a_restored_member_the_roster_no_longer_lists_starts_no_run_before_it_is_retired`]:
/// the state directory its first boot runs in.
const DROPPED_CHILD_STATE: &str = "META_CUSTOMIZER_TOOLS_DROPPED_CHILD_STATE";
/// A member the first boot runs and the restart's roster no longer lists.
const DROPPED: &str = "personal:bob";
/// The work [`DroppedMemberProbe`] submits to [`DROPPED`]'s member.
const PROBE: &str = "work for the member the roster no longer lists";
/// How long [`DroppedMemberProbe`] waits for that work to start a run.
const DROPPED_RUN_BOUND: Duration = Duration::from_secs(3);

/// A host customizer that declares [`TOOL`]. On its first build that carries
/// the mob handle (the identity bootstrap's publication for the restored
/// members it owes, which runs before the roster pass) it submits work to the
/// member of `target`, which the build revived, and records how the submit
/// went and whether the work started a run within [`DROPPED_RUN_BOUND`].
struct DroppedMemberProbe {
    capture: CaptureClient,
    target: &'static str,
    probe: std::sync::Mutex<Option<(String, bool)>>,
}

#[async_trait]
impl AgentCustomizer for DroppedMemberProbe {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        if let Some(handle) = context.runtime_services.mob_handle()
            && self.probe.lock().unwrap().is_none()
        {
            let member_id = meerkat_meta::member_comms_id::mob_member_id(self.target);
            let submitted = match handle.member(&member_id).await {
                Ok(member) => match tokio::time::timeout(
                    Duration::from_secs(10),
                    member.send(
                        meerkat_core::ContentInput::Text(PROBE.to_string()),
                        meerkat_core::types::HandlingMode::Queue,
                    ),
                )
                .await
                {
                    Ok(Ok(_)) => "admitted".to_string(),
                    Ok(Err(error)) => format!("refused: {error}"),
                    Err(_) => "unanswered".to_string(),
                },
                Err(error) => format!("no member: {error}"),
            };
            let deadline = Instant::now() + DROPPED_RUN_BOUND;
            while !self.capture.saw(PROBE) && Instant::now() < deadline {
                sleep(Duration::from_millis(50)).await;
            }
            *self.probe.lock().unwrap() = Some((submitted, self.capture.saw(PROBE)));
        }
        draft.local_external_tools = LocalExternalToolOverlay::new(Arc::new(HostTools));
        Ok(())
    }
}

/// After an unclean stop meerkat's resume revives every member the first
/// boot ran while the mob is built, the one whose identity the restart's
/// roster no longer lists as well. Nothing publishes that member's customizer
/// tools: the build holds its run starts, so work submitted to it before the
/// identity bootstrap's roster pass retires it starts no run.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_member_the_roster_no_longer_lists_starts_no_run_before_it_is_retired() {
    const MOB_ID: &str = "customizer-tools-restart-f";
    if let Some(state) = std::env::var_os(DROPPED_CHILD_STATE) {
        // The child: the first boot runs both members, then exits without
        // shutdown.
        let runtime = boot_listing(
            MOB_ID,
            Path::new(&state),
            CaptureClient::default(),
            Arc::new(ToolCustomizer::default()),
            IdentityBootstrapMode::EagerMaterialize,
            &[MEMBER, DROPPED],
        )
        .await;
        let identity_runtime = runtime.identity_runtime().expect("identity runtime");
        for name in [MEMBER, DROPPED] {
            let status = identity_runtime.status(&id(name)).await.expect("status");
            assert!(status.session_id.is_some(), "{name} runs: {status:?}");
        }
        sleep(Duration::from_millis(500)).await;
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

    let capture = CaptureClient::default();
    let probe = Arc::new(DroppedMemberProbe {
        capture: capture.clone(),
        target: DROPPED,
        probe: std::sync::Mutex::new(None),
    });
    let runtime = boot_listing(
        MOB_ID,
        &state,
        capture.clone(),
        probe.clone(),
        IdentityBootstrapMode::LazyMaterialize,
        &[MEMBER],
    )
    .await;
    let (submitted, ran) = probe
        .probe
        .lock()
        .unwrap()
        .clone()
        .expect("the identity bootstrap's publication ran the probe before the roster pass");
    assert!(
        !ran,
        "the restored member the roster no longer lists started no run before the roster pass \
         retired it (the work submitted to it: {submitted})"
    );
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
    sleep(Duration::from_millis(500)).await;
    assert!(
        !capture.saw(PROBE),
        "the work submitted to the retired member never ran"
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
/// pass never retires it. Waits (bounded) until Core's reconciler reports it
/// converged.
async fn adopt(runtime: &meerkat_meta::UnifiedRuntime, mob_id: &str, name: &str) {
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
/// lists it. Meerkat's resume revives the member while the mob is built, as
/// it revives every member after an unclean stop. The roster pass never
/// retires a host-adopted identity and no roster publication covers it, so
/// a hold staged for it would never be released: the mob build holds none of
/// its run starts, and once Core's reconciler admits work to it again, its
/// runs start as before the restart.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_adopted_member_the_roster_no_longer_lists_is_not_held_after_a_restart() {
    const MOB_ID: &str = "customizer-tools-restart-g";
    if let Some(state) = std::env::var_os(ADOPTED_CHILD_STATE) {
        // The child: the first boot runs both members, a host adopts one,
        // then the process exits without shutdown.
        let runtime = boot_listing(
            MOB_ID,
            Path::new(&state),
            CaptureClient::default(),
            Arc::new(ToolCustomizer::default()),
            IdentityBootstrapMode::EagerMaterialize,
            &[MEMBER, ADOPTED],
        )
        .await;
        adopt(&runtime, MOB_ID, ADOPTED).await;
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

    let runtime = boot_listing(
        MOB_ID,
        &state,
        CaptureClient::default(),
        Arc::new(ToolCustomizer::default()),
        IdentityBootstrapMode::LazyMaterialize,
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
/// a shutdown (a crash): its identity lease is never released. The next
/// process revives the member while its mob is built, and Core's
/// reconciler must converge the adopted identity again before it admits
/// work. That process holds the lease under the same holder, so it
/// reclaims the crashed predecessor's lease at once instead of waiting for
/// it to expire: the identity converges within
/// [`CRASH_RESTART_CONVERGENCE_BOUND`] of the build, and work submitted to
/// it then runs. The row logs a UTC timeline (the child's exit, the
/// build's return, each poll, the first converged read, the first admitted
/// submit, the run) for up to [`CRASH_RESTART_OBSERVATION`].
#[tokio::test(flavor = "multi_thread")]
async fn a_host_adopted_member_converges_and_runs_promptly_after_a_crash_restart() {
    const MOB_ID: &str = "customizer-tools-restart-h";
    const WORK: &str = "work for the host-adopted member after a crash restart";
    if let Some(state) = std::env::var_os(CRASH_ADOPTED_CHILD_STATE) {
        // The child: the first boot runs both members, a host adopts one,
        // then the process exits without shutdown.
        let runtime = boot_listing(
            MOB_ID,
            Path::new(&state),
            CaptureClient::default(),
            Arc::new(ToolCustomizer::default()),
            IdentityBootstrapMode::EagerMaterialize,
            &[MEMBER, ADOPTED],
        )
        .await;
        adopt(&runtime, MOB_ID, ADOPTED).await;
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
/// What each worker's run works on.
const WORKER_TURN: &str = "WORKER-RUN-OSCAR-9";
/// The tool call each worker's run commits its first round with.
const WORKER_ROUND_CALL: &str = "call-worker-round";
/// Set in the child process of
/// [`durable_workers_spawned_at_runtime_continue_after_a_kill`]: the state
/// directory its first boot runs in.
const SPAWNED_CHILD_STATE: &str = "META_SPAWNED_DURABLE_WORKERS_CHILD_STATE";

fn worker_turn(worker: &str) -> String {
    format!("{WORKER_TURN} for {worker}")
}

/// The model of the spawned-worker rows. A worker run's first call asks for
/// a tool no catalog has (refused per call), so its round commits; with
/// `hold`, the call after that round never answers, so the run stays open.
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

/// A runtime of the spawned-worker rows over `state`, whose roster lists
/// `roster`.
async fn boot_spawning(
    mob_id: &str,
    state: &Path,
    model: WorkerModel,
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
            .agent_customizer(Arc::new(ToolCustomizer::default()))
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
/// handle, as a member's mob tools spawn them, from the durable worker
/// profile. Each worker's run commits a tool round and then waits on its
/// model.
async fn boot_with_running_workers(mob_id: &str, state: &Path) -> meerkat_meta::UnifiedRuntime {
    let model = WorkerModel::holding();
    let runtime = boot_spawning(mob_id, state, model.clone(), &[MEMBER, DROPPED]).await;
    let handle = runtime.mob_runtime().handle();
    for worker in WORKERS {
        let identity = meerkat_mob::AgentIdentity::from(worker);
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
                "worker",
                identity.clone(),
            ))
            .await
            .unwrap_or_else(|error| panic!("spawn {worker}: {error}"));
        handle
            .member(&identity)
            .await
            .unwrap_or_else(|error| panic!("{worker}'s member: {error}"))
            .send(
                meerkat_core::ContentInput::Text(worker_turn(worker)),
                meerkat_core::types::HandlingMode::Queue,
            )
            .await
            .unwrap_or_else(|error| panic!("{worker}'s run starts: {error}"));
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
    let runtime = boot_spawning(mob_id, state, model.clone(), &[MEMBER]).await;
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
/// `first` boots and stops; `control` restarts over the same comms identity
/// root and reads everything healthy; `empty` restarts over an empty one,
/// so Core restores the member broken, and reads it broken everywhere.
async fn broken_member_step(mob_id: &str, step: &str, state: &Path) {
    let runtime = boot(
        mob_id,
        state,
        CaptureClient::default(),
        Arc::new(ToolCustomizer::default()),
        IdentityBootstrapMode::EagerMaterialize,
    )
    .await;
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
