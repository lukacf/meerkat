//! LazyMaterialize cross-boot recall guard (production field report, 2026-07-29:
//! "silent resume context loss").
//!
//! # The coverage hole this closes
//!
//! Two deterministic guards cover the EAGER bootstrap arm
//! (`identity_first_cold_restart_continuity`,
//! `identity_first_respawn_continuity`) and both are green. Nothing paired the
//! LAZY arm with a recall assertion, and the two arms diverge in
//! `apply_roster_controlled`: `EagerMaterialize` runs
//! `orchestrator::restore_flow` (resume at bootstrap), while
//! `LazyMaterialize | LazyWithBackgroundWarm` run
//! `orchestrator::lazy_register_flow`, which registers the roster and asks
//! Core for no session. The member wakes at the first `send`/`dispatch` (or
//! from the background warmer) through the shared `embody_identity` door.
//!
//! The field report: boot A takes a turn and the post-turn document lands in
//! the store with correctly-ordered messages; after a kill -9 and boot B every
//! resume signature is clean (0 conflicts, 0 held, 0 quarantined, 0 Broken),
//! yet the agent answers the next message with no record of the earlier
//! exchange. The store is right and the model's working context was empty -
//! completely silent, nothing in logs. So the property to pin is not a status
//! or a store row: it is the BYTES OF THE POST-RESTART LLM REQUEST.
//!
//! # Harness shape
//!
//! Every boot composes the runtime the gateways run: `persistent_state()`,
//! the state directory's identity substrate (`continuity_from_state_dir`) and
//! a roster provider. The transcript lives in Core's session store on
//! `runtime.sqlite`, so every boot first requires a durable session there
//! (`assert_durable_core_session`) before asserting behavior: a mis-wired
//! harness fails as MIS-WIRED instead of passing while proving nothing.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, RosterProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, CustomizerError,
    DurableAgentSpec, IdentityLifecycleState, RosterContext, RosterError,
};
use meerkat_meta::{IdentityBootstrapMode, UnifiedRuntimeBuilder};
use meerkat_mob::{MobDefinition, ProfileName};
use tokio::time::sleep;

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const MOB_TOML: &str = r#"
[mob]
id = "lazy-recall-continuity"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

const MEMBER: &str = "personal:alice";
const RUNTIME_INSTANCE: &str = "lazy-recall-continuity";

/// Serializes this binary's tests: in the parent one memo-free child runs at a
/// time, and inside a child its single test owns the process.
static SERIAL_WINDOW: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Set on the re-exec'd child so it runs the test body instead of re-execing
/// again.
const MEMO_FREE_CHILD: &str = "MEERKAT_LAZY_RECALL_MEMO_FREE_CHILD";

/// Run this test in a child process with meerkat's PROCESS-GLOBAL memos
/// disabled; returns `true` in the parent (which has just proxied the whole
/// test) and `false` in the child (which must run the body).
///
/// # Why every test here needs it
///
/// Integration tests in one binary share one process, and meerkat keeps memos
/// that outlive a runtime: the transcript-graph decode memo, the digest
/// accumulator's memo, and the slim-materialization snapshot registry that
/// `SessionHead::from_session` seeds on the PRODUCER side of a save (see
/// `identity_first_subprocess_reboot.rs`, which exists because of them). Any
/// of those can hand a later same-process "boot" a decoded, digested or fully
/// materialized transcript that a real `execve` could never inherit —
/// precisely how a legacy-decode or crash-recovery defect hides behind a green
/// same-process test. `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` is the established
/// kill switch for all of them.
///
/// It cannot be set in-process: `unsafe_code` is FORBIDDEN workspace-wide and
/// `std::env::set_var` is unsafe on edition 2024. So this follows the sibling
/// subprocess test's idiom — put it in a CHILD's environment — by re-execing
/// this test binary for the one named test. The child's output is forwarded, so
/// a failure reads exactly as it would have in-process.
fn proxied_to_memo_free_child(test_name: &str) -> bool {
    if std::env::var_os(MEMO_FREE_CHILD).is_some() {
        return false;
    }
    let exe = std::env::current_exe().expect("this test binary's own path");
    let output = std::process::Command::new(exe)
        // `--include-ignored` matters: without it a child running an
        // `#[ignore]`d test would filter it out and exit 0, and the parent
        // would report a PASS for a test that never ran.
        .args([
            test_name,
            "--exact",
            "--nocapture",
            "--test-threads=1",
            "--include-ignored",
        ])
        .env(MEMO_FREE_CHILD, "1")
        .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
        .output()
        .expect("spawn the memo-free child");
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "the memo-free child running `{test_name}` failed ({}); its output is above",
        output.status
    );
    true
}

/// A system prompt only the customizer can install: the profile has none, so
/// its presence in a captured request means `customize_build` ran for that
/// member's build.
const CUSTOMIZER_PROMPT: &str = "You are alice. CUSTOMIZER-PROMPT-MARKER-4-QUEBEC.";

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("parse identity")
}

fn definition() -> MobDefinition {
    MobDefinition::from_toml(MOB_TOML).expect("parse lazy-recall mob definition")
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

struct OneMemberRoster;

#[async_trait]
impl RosterProvider for OneMemberRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![durable_spec(MEMBER)])
    }
}

/// Installs a marker system prompt at build time.
///
/// On the lazy arm the customizer is not passed to `lazy_register_flow` at
/// all; `embody_identity` reads it from runtime state (installed by the
/// builder via `set_agent_customizer`) when the first send materializes the
/// member. A lazily materialized agent losing its host prompt would be a
/// second, independent defect, so both boots assert the marker.
struct MarkerPromptCustomizer;

#[async_trait]
impl AgentCustomizer for MarkerPromptCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        draft.system_prompt = Some(CUSTOMIZER_PROMPT.to_string());
        Ok(())
    }
}

/// The raw token counts one [`CaptureClient`] turn reports.
///
/// meerkat 0.8.22 made a turn that reports NO usage fatal, and silently so at
/// compile time: the adapter seeds its local usage with `Usage::default()`
/// (no accounting) and `commit_calling_llm_response` then runs
/// `TurnUsage::try_from_usage` on whatever the stream left there, failing the
/// turn with `normalized_provider_accounting_unavailable`. Under 0.8.21 that
/// cost nothing here - emission was guarded on `usage_input_tokens > 0`,
/// which is never raised above its zero default in this file, and the
/// tool-call branch emitted no usage at all, so in practice NO turn here
/// reported usage. A type-only port would therefore have compiled and then
/// failed EVERY turn, surfacing as a provider error nowhere near anything
/// that mentions usage. Hence every completing turn now reports, whatever the
/// count. Declaring zero input tokens preserves the 0.8.21 value and stays
/// inert for compaction, which documents "no compaction when
/// `last_input_tokens` is zero".
fn capture_usage(input_tokens: u64) -> meerkat_core::types::Usage {
    meerkat_core::types::Usage {
        input_tokens,
        output_tokens: 1,
        ..Default::default()
    }
}

/// Records the serialized LLM request each turn and answers "ok" so turns
/// complete without a real provider.
///
/// `hang` turns it into a mid-turn stall: the request is still recorded, but
/// the stream then waits for [`CaptureClient::release`] instead of answering,
/// so the turn stays IN FLIGHT and no boundary save can land.
#[derive(Clone)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    hang: Arc<AtomicBool>,
    /// Consumed by the first hanging turn, which spends it on a TOOL ROUND
    /// TRIP so the turn has in-flight work (and therefore durable intra-turn
    /// state) before it dies.
    tool_step: Arc<AtomicBool>,
    gate: Arc<tokio::sync::Semaphore>,
    /// Provider-reported input tokens. `last_input_tokens` (the compaction
    /// trigger, `compact.rs`: "compaction triggers when `last_input_tokens >=
    /// auto_compact_threshold`") comes from exactly this, so raising it would
    /// make compaction fire deterministically instead of depending on
    /// transcript size. No test in this file raises it: it stays at zero, which
    /// is the "never compact" arm of that same rule. The turn still REPORTS
    /// that zero - see [`capture_usage`] for why reporting nothing is no
    /// longer an option.
    usage_input_tokens: Arc<std::sync::atomic::AtomicU64>,
}

impl Default for CaptureClient {
    fn default() -> Self {
        Self {
            requests: Arc::new(std::sync::Mutex::new(Vec::new())),
            hang: Arc::new(AtomicBool::new(false)),
            tool_step: Arc::new(AtomicBool::new(false)),
            // Starts closed; only a hanging stream ever waits on it.
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            usage_input_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

impl CaptureClient {
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn last(&self) -> Option<String> {
        self.requests.lock().unwrap().last().cloned()
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
        let hang = self.hang.load(Ordering::SeqCst);
        // One tool round trip before the stall: a turn that dies with NO
        // in-flight work has nothing durable to leave behind, and a mid-turn
        // kill would then be indistinguishable from a pre-turn one.
        let tool_step = hang && self.tool_step.swap(false, Ordering::SeqCst);
        let gate = self.gate.clone();
        let usage_input_tokens = self.usage_input_tokens.load(Ordering::SeqCst);
        // meerkat 0.8.22 rejects a turn whose stream carried no normalized
        // provider accounting, so the terminal `Done` never travels alone -
        // and that includes the tool-call turn, whose only outcome is a tool
        // call. Both pairs are built out here, against `request`, so the
        // stream body never borrows it.
        //
        // `Provider::OpenAI` is passed as the CLIENT's own declaration, not as
        // the answer: the helper resolves the accounting provider from the
        // MODEL through the same catalog authority `AgentFactory` used, and
        // falls back to the declaration only for an uncatalogued model. That
        // distinction is load-bearing here, because `provider()` below never
        // reaches the factory at all - Meta wraps the mob-wide default
        // client in `ProviderAgnosticLlmClient`, which reports
        // `Provider::Other` precisely so one stub can serve members across
        // providers. A hardcoded `OpenAI` would bind fine and then fail every
        // turn with `normalized_provider_accounting_identity_mismatch` the day
        // a profile in this file moves to a `claude-*` model.
        let [tool_usage, tool_done] = llm_usage::usage_then_done_with(
            request,
            meerkat::Provider::OpenAI,
            capture_usage(usage_input_tokens),
            StopReason::ToolUse,
        );
        let [turn_usage, turn_done] = llm_usage::usage_then_done_with(
            request,
            meerkat::Provider::OpenAI,
            capture_usage(usage_input_tokens),
            StopReason::EndTurn,
        );
        Box::pin(async_stream::stream! {
            if tool_step {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: "midturn-peers-1".to_string(),
                    name: "peers".to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                });
                yield Ok(tool_usage);
                yield Ok(tool_done);
                return;
            }
            if hang {
                // Mid-turn stall: the request is already recorded (so the
                // barrier can see the turn started) and the turn never reaches
                // its boundary save. Nothing at all is emitted while stalled -
                // usage included - because a stalled turn has no boundary to
                // account for; `release` then lets it finish through the tail
                // below like any other turn.
                let _permit = gate.acquire().await;
            }
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(turn_usage);
            yield Ok(turn_done);
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

// ---------------------------------------------------------------------------
// Gateway-shaped harness
// ---------------------------------------------------------------------------

/// Build the gateway-shaped runtime (persistent state, the state directory's
/// identity substrate, a roster provider) under the requested bootstrap mode.
async fn boot(
    state: &Path,
    capture: CaptureClient,
    mode: IdentityBootstrapMode,
) -> meerkat_meta::UnifiedRuntime {
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition())
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(OneMemberRoster))
        .agent_customizer(Arc::new(MarkerPromptCustomizer))
        .identity_bootstrap_mode(mode)
        .identity_runtime_instance_id(RUNTIME_INSTANCE)
        .comms(true)
        .default_llm_client(Arc::new(capture));
    Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime")
}

/// The marker carried by [`PROFILE_PROMPT_MOB_TOML`]'s inline skill. Its
/// presence in a captured request proves the profile-level prompt carrier is
/// real, so the revival assertions rest on a prompt that exists.
const PROFILE_PROMPT_MARKER: &str = "PROFILE-PROMPT-MARKER-16-YANKEE";

/// The lazy-recall mob with a PROFILE-declared system prompt - the one
/// configuration shape proven (field contrast, 2026-08-06) to
/// re-author one assembled System row per automatic revival on the
/// 0.8.16-0.8.18 line: the assembled prompt lands in persisted spawn state
/// at original spawn and the mob-resume AppendExplicit branches re-lower it
/// as fresh explicit intent on every boot.
///
/// The prompt rides the only profile-level carrier meerkat reads: `skills`
/// resolved against an inline `[skills.<id>]`. Until 2026-09-03 this fixture
/// wrote a bare `system_prompt` key under `[profiles.personal]`, which is not
/// a `Profile` field and is dropped at `MobDefinition::from_toml`, so no
/// prompt was in play and the "authors no extra rows" assertions were
/// vacuous.
const PROFILE_PROMPT_MOB_TOML: &str = r#"
[mob]
id = "lazy-recall-continuity"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
skills = ["personal_role"]

[profiles.personal.tools]
comms = true

[skills.personal_role]
source = "inline"
content = "PROFILE-PROMPT-MARKER-16-YANKEE: you are alice."
"#;

/// `boot` with an explicit definition and NO agent customizer, so a
/// profile-declared prompt is the only prompt source in play.
async fn boot_with_definition_no_customizer(
    state: &Path,
    capture: CaptureClient,
    mode: IdentityBootstrapMode,
    definition: MobDefinition,
) -> meerkat_meta::UnifiedRuntime {
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition)
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(OneMemberRoster))
        .identity_bootstrap_mode(mode)
        .identity_runtime_instance_id(RUNTIME_INSTANCE)
        .comms(true)
        .default_llm_client(Arc::new(capture));
    Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime")
}

// ---------------------------------------------------------------------------
// Precondition probe
// ---------------------------------------------------------------------------

/// What Core's session store durably holds: `(session_id, message_count)`
/// for every session, ordered by id. Sessions live in Core's own session
/// store, which shares `runtime.sqlite` with the runtime store.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CoreCensus {
    heads: Vec<(String, i64)>,
}

async fn core_census(state: &Path) -> CoreCensus {
    use meerkat::SessionStore as _;
    let path = state.join("runtime.sqlite");
    if !path.exists() {
        return CoreCensus::default();
    }
    let store = meerkat_store::SqliteSessionStore::open(path).expect("open Core's session store");
    let mut heads = store
        .list(meerkat_core::SessionFilter::default())
        .await
        .expect("list Core's sessions")
        .into_iter()
        .map(|meta| {
            (
                meta.id.to_string(),
                i64::try_from(meta.message_count).expect("a message count"),
            )
        })
        .collect::<Vec<_>>();
    heads.sort();
    CoreCensus { heads }
}

/// THE PRECONDITION: Core's session store holds a durable session, so the
/// assertions after it read real state.
async fn assert_durable_core_session(state: &Path, phase: &str) -> CoreCensus {
    let census = core_census(state).await;
    assert!(
        !census.heads.is_empty(),
        "PRECONDITION FAILED ({phase}): Core's session store on runtime.sqlite holds no \
         session; census: {census:?}"
    );
    census
}

/// Turn-boundary barrier: wait until Core's session store holds exactly one
/// session with at least `floor` messages, returning `(session_id, count)`.
async fn wait_for_durable_document_at_least(state: &Path, floor: i64, what: &str) -> (String, i64) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let census = core_census(state).await;
        if let [(session, count)] = census.heads.as_slice()
            && *count >= floor
        {
            return (session.clone(), *count);
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: want one durable session with >= {floor} messages, \
             census: {census:?}"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_turn(capture: &CaptureClient, want: usize, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while capture.count() < want {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what} to reach the LLM"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The production report, deterministically: with `LazyMaterialize`, a restart that
/// materializes the member ON FIRST SEND must replay the persisted transcript
/// into the LLM request.
///
/// Both boots prove the lazy arm was genuinely taken rather than silently
/// falling back to eager restore: the bootstrap pass reports the member
/// Dormant (`ready == false`, `counts.dormant == 1`) with no session yet, and
/// the identity only becomes Active AFTER the send. Without those assertions a
/// green run could be the eager path wearing a lazy label, proving nothing.
#[tokio::test(flavor = "multi_thread")]
async fn lazy_materialize_cross_boot_recall_replays_the_transcript() {
    const TOKEN: &str = "MARKER-LAZY-RECALL-3-XRAY";
    if proxied_to_memo_free_child("lazy_materialize_cross_boot_recall_replays_the_transcript") {
        return;
    }
    let _serial = SERIAL_WINDOW.lock().await;
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let member = id(MEMBER);

    // --- Boot 1: lazy register, then materialize on demand from turn 1 ---
    let session_id;
    {
        let capture = CaptureClient::default();
        let runtime = boot(
            &state,
            capture.clone(),
            IdentityBootstrapMode::LazyMaterialize,
        )
        .await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();

        let bootstrap = identity_runtime.identity_bootstrap_status();
        eprintln!("PROBE boot 1 bootstrap: {bootstrap:?}");
        assert_eq!(
            bootstrap.mode,
            IdentityBootstrapMode::LazyMaterialize,
            "the runtime must be running the lazy arm; bootstrap: {bootstrap:?}"
        );
        assert!(
            bootstrap.complete,
            "the lazy bootstrap pass must have finished; bootstrap: {bootstrap:?}"
        );
        assert!(
            !bootstrap.ready,
            "lazy bootstrap must leave the roster UNmaterialized (ready == false); \
             bootstrap: {bootstrap:?}"
        );
        assert_eq!(
            (bootstrap.counts.dormant, bootstrap.counts.active),
            (1, 0),
            "lazy bootstrap must register the member dormant and materialize nothing; \
             bootstrap: {bootstrap:?}"
        );
        let before = identity_runtime
            .status(&member)
            .await
            .expect("dormant identity is inspectable");
        assert_eq!(
            before.state,
            IdentityLifecycleState::Dormant,
            "boot 1 must start Dormant, not eagerly restored; status: {before:?}"
        );

        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text(format!("Please note this token: {TOKEN}")),
            )
            .await
            .expect("send turn 1");
        wait_for_turn(&capture, 1, "boot 1's turn").await;

        let after = identity_runtime
            .status(&member)
            .await
            .expect("status after the first send");
        assert_eq!(
            after.state,
            IdentityLifecycleState::Active,
            "the FIRST SEND is what must have materialized the member (on-demand \
             materialization from Dormant); status: {after:?}"
        );
        session_id = after
            .session_id
            .as_ref()
            .expect("a materialized identity owns a durable session")
            .to_string();

        // The customizer must reach a freshly materialized member, otherwise
        // the post-restart prompt assertion below would be vacuous.
        let first = capture.last().expect("a boot-1 request was captured");
        assert!(
            first.contains(CUSTOMIZER_PROMPT),
            "the customizer's system prompt must reach the lazily materialized member's \
             first request; request: {first}"
        );

        let (head, count) =
            wait_for_durable_document_at_least(&state, 2, "boot 1's turn to commit").await;
        eprintln!("PROBE boot 1 durable head: {head} at {count} messages");
        assert_eq!(
            head, session_id,
            "the durable document must belong to the materialized session"
        );
        // Retire the identity before the stop: Core keeps its history, so boot
        // 2 registers it Dormant and the first send's lazy wake resumes this
        // session. A member left live at a clean stop is restored live by
        // Core's mob at the next start, which the lazy arm cannot make dormant.
        identity_runtime
            .retire(&member)
            .await
            .expect("retire the identity before the stop");
        runtime.shutdown().await;
    }

    let census = assert_durable_core_session(&state, "boot 1 + one turn").await;
    eprintln!("PROBE census after boot 1: {census:?}");
    assert!(
        census
            .heads
            .iter()
            .any(|(session, _)| session == &session_id),
        "Core's session store must hold the session the turn was written to; \
         census: {census:?}"
    );

    // --- Boot 2: lazy again. First send must materialize AND recall. ---
    {
        let capture = CaptureClient::default(); // fresh: only boot-2 requests
        let runtime = boot(
            &state,
            capture.clone(),
            IdentityBootstrapMode::LazyMaterialize,
        )
        .await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();

        let pre = assert_durable_core_session(&state, "boot 2 (pre-turn)").await;
        assert!(
            pre.heads.iter().any(|(session, _)| session == &session_id),
            "boot 2 must still find the boot-1 session in Core's store; census: {pre:?}"
        );

        let bootstrap = identity_runtime.identity_bootstrap_status();
        eprintln!("PROBE boot 2 bootstrap: {bootstrap:?}");
        assert_eq!(
            (bootstrap.counts.dormant, bootstrap.counts.active),
            (1, 0),
            "boot 2 must ALSO take the lazy arm (dormant registration, nothing \
             materialized) — otherwise this test silently exercises eager restore; \
             bootstrap: {bootstrap:?}"
        );
        let before = identity_runtime
            .status(&member)
            .await
            .expect("dormant identity is inspectable after restart");
        assert_eq!(
            before.state,
            IdentityLifecycleState::Dormant,
            "boot 2 must start Dormant: registered, with no member yet; status: {before:?}"
        );

        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send turn 2");
        wait_for_turn(&capture, 1, "the post-restart turn").await;

        let last = capture.last().expect("a post-restart request was captured");
        // THE PROPERTY THE FIELD REPORT FOUND VIOLATED. The store document is
        // correct in the field; what was empty was the model's working
        // context, so this asserts the REQUEST BYTES, not a status or a row.
        assert!(
            last.contains(TOKEN),
            "post-restart LLM request must replay the persisted transcript (token {TOKEN}); \
             on-demand materialization served an empty or stale document. Durable census: \
             {pre:?}. Request: {last}"
        );
        // Independent second property: resume AUTHORS NOTHING (the bridge
        // clears the prompt override; meerkat preserves every ordered System
        // byte-for-byte) — so this pins that the host prompt survives a lazy
        // restart purely as preserved durable transcript, not by any
        // boot-time re-application.
        assert!(
            last.contains(CUSTOMIZER_PROMPT),
            "post-restart LLM request must still carry the host system prompt; \
             request: {last}"
        );

        let after = identity_runtime
            .status(&member)
            .await
            .expect("status after the post-restart send");
        assert_eq!(
            after.state,
            IdentityLifecycleState::Active,
            "the post-restart send must have materialized the member; status: {after:?}"
        );
        assert_eq!(
            after.session_id.as_ref().map(ToString::to_string),
            Some(session_id.clone()),
            "on-demand materialization must RESUME the durable session, not create a \
             fresh one; status: {after:?}"
        );
        runtime.shutdown().await;
    }
}

/// Same property for `LazyWithBackgroundWarm`, which shares the
/// `lazy_register_flow` match arm and then materializes from the background
/// warmer instead of from the first send. Cheap to cover and a different
/// materialization trigger over the same dormant registration.
#[tokio::test(flavor = "multi_thread")]
async fn lazy_with_background_warm_cross_boot_recall_replays_the_transcript() {
    const TOKEN: &str = "MARKER-LAZY-WARM-8-SIERRA";
    if proxied_to_memo_free_child(
        "lazy_with_background_warm_cross_boot_recall_replays_the_transcript",
    ) {
        return;
    }
    let _serial = SERIAL_WINDOW.lock().await;
    let mode = IdentityBootstrapMode::LazyWithBackgroundWarm { concurrency: 1 };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let member = id(MEMBER);

    // --- Boot 1: background warm materializes, then turn 1 ---
    let session_id;
    {
        let capture = CaptureClient::default();
        let runtime = boot(&state, capture.clone(), mode.clone()).await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();

        let (bootstrap, timed_out) = identity_runtime
            .wait_identity_bootstrap_terminal(Duration::from_secs(30))
            .await;
        eprintln!("PROBE warm boot 1 bootstrap: {bootstrap:?}");
        assert!(
            !timed_out,
            "background warm must reach a terminal state; bootstrap: {bootstrap:?}"
        );
        assert_eq!(bootstrap.mode, mode, "bootstrap: {bootstrap:?}");
        assert_eq!(
            (bootstrap.counts.active, bootstrap.counts.broken),
            (1, 0),
            "the warmer must materialize the roster it registered dormant; \
             bootstrap: {bootstrap:?}"
        );

        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text(format!("Please note this token: {TOKEN}")),
            )
            .await
            .expect("send turn 1");
        wait_for_turn(&capture, 1, "warm boot 1's turn").await;

        let first = capture.last().expect("a warm boot-1 request was captured");
        assert!(
            first.contains(CUSTOMIZER_PROMPT),
            "the customizer's system prompt must reach the background-warmed member; \
             request: {first}"
        );

        let (head, count) =
            wait_for_durable_document_at_least(&state, 2, "warm boot 1's turn to commit").await;
        eprintln!("PROBE warm boot 1 durable head: {head} at {count} messages");
        session_id = head;
        runtime.shutdown().await;
    }

    // --- Boot 2: warm again, then a turn that must recall turn 1 ---
    {
        let capture = CaptureClient::default(); // fresh: only boot-2 requests
        let runtime = boot(&state, capture.clone(), mode.clone()).await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();

        let pre = assert_durable_core_session(&state, "warm boot 2 (pre-turn)").await;
        assert!(
            pre.heads.iter().any(|(session, _)| session == &session_id),
            "warm boot 2 must still find the boot-1 session in Core's store; census: {pre:?}"
        );

        let (bootstrap, timed_out) = identity_runtime
            .wait_identity_bootstrap_terminal(Duration::from_secs(30))
            .await;
        eprintln!("PROBE warm boot 2 bootstrap: {bootstrap:?}");
        assert!(
            !timed_out,
            "background warm must reach a terminal state after a restart; \
             bootstrap: {bootstrap:?}"
        );
        assert_eq!(
            (bootstrap.counts.active, bootstrap.counts.broken),
            (1, 0),
            "the warmer must re-materialize the persisted member; bootstrap: {bootstrap:?}"
        );

        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send turn 2");
        wait_for_turn(&capture, 1, "the post-restart warm turn").await;

        let last = capture
            .last()
            .expect("a post-restart warm request was captured");
        assert!(
            last.contains(TOKEN),
            "post-restart LLM request must replay the persisted transcript (token {TOKEN}); \
             background-warm materialization served an empty or stale document. Durable \
             census: {pre:?}. Request: {last}"
        );
        assert!(
            last.contains(CUSTOMIZER_PROMPT),
            "post-restart warm request must still carry the host system prompt; \
             request: {last}"
        );

        let after = identity_runtime
            .status(&member)
            .await
            .expect("status after the post-restart warm send");
        assert_eq!(
            after.session_id.as_ref().map(ToString::to_string),
            Some(session_id.clone()),
            "background-warm materialization must RESUME the durable session; \
             status: {after:?}"
        );
        runtime.shutdown().await;
    }
}

/// The lazy runtime with the marker customizer over persistent state; its
/// sessions live in Core's own stores on `runtime.sqlite`.
async fn boot_lazy_runtime(state: &Path, capture: CaptureClient) -> meerkat_meta::UnifiedRuntime {
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition())
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(OneMemberRoster))
        .agent_customizer(Arc::new(MarkerPromptCustomizer))
        .identity_bootstrap_mode(IdentityBootstrapMode::LazyMaterialize)
        .identity_runtime_instance_id(RUNTIME_INSTANCE)
        .comms(true)
        .default_llm_client(Arc::new(capture));
    Box::pin(builder.build())
        .await
        .expect("build the lazy UnifiedRuntime")
}

/// The PR #304 CI wedge, mirrored at the gateway shape (the Python
/// `test_real_gateway_reset_reprofile_materializes_shell_tools` sequence):
/// an identity with an ACTIVE session takes a LIVE reset (the identity is
/// bound to a fresh session while the superseded runtime session is still live in
/// this process; its retirement is deferred cleanup debt BY DESIGN), a real
/// turn runs on the fresh session, and shutdown completes within a bounded
/// horizon. Before the supersede-absorbing projection, the superseded
/// session's next boundary commit (its shutdown checkpoint at the latest)
/// failed the durable write-through with the store's cursor refusal, the
/// runtime escalated to repair-blocked retention, the deferred retire
/// wedged behind it, and gateway shutdown blew its 310s bounded horizon.
#[tokio::test(flavor = "multi_thread")]
async fn live_reset_takes_a_turn_and_shuts_down_within_horizon() {
    if proxied_to_memo_free_child("live_reset_takes_a_turn_and_shuts_down_within_horizon") {
        return;
    }
    let _serial = SERIAL_WINDOW.lock().await;
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let member = id(MEMBER);
    let capture = CaptureClient::default();
    let runtime = boot_lazy_runtime(&state, capture.clone()).await;
    let identity_runtime = runtime
        .identity_runtime()
        .expect("identity runtime")
        .clone();
    identity_runtime
        .send(
            &member,
            &meerkat_core::ContentInput::Text("turn on the ORIGINAL session".to_string()),
        )
        .await
        .expect("the pre-reset turn");
    wait_for_turn(&capture, 1, "the pre-reset turn").await;
    let before = identity_runtime
        .status(&member)
        .await
        .expect("status before reset");
    let old_session = before
        .session_id
        .as_ref()
        .map(ToString::to_string)
        .expect("a bound session before reset");

    let record = identity_runtime
        .reset(&member)
        .await
        .expect("the LIVE reset");
    assert_ne!(
        record.session_id().map(ToString::to_string).as_deref(),
        Some(old_session.as_str()),
        "reset must mint a fresh session"
    );

    identity_runtime
        .send(
            &member,
            &meerkat_core::ContentInput::Text("turn on the FRESH session".to_string()),
        )
        .await
        .expect("the post-reset turn on the fresh session");
    wait_for_turn(&capture, 2, "the post-reset turn").await;

    // The horizon pin. In the field the superseded session's failed
    // projection held teardown in retain-for-retry past the gateway's 310s
    // bounded horizon; a healthy teardown completes in seconds locally but a
    // loaded 2-vCPU CI runner needs minutes for the full lifecycle - 5 minutes
    // still cleanly separates slow-but-done from the 310s+ retention wedge.
    tokio::time::timeout(std::time::Duration::from_mins(5), runtime.shutdown())
        .await
        .expect(
            "shutdown must complete within the bounded horizon; a timeout here is the \
             superseded-session repair-blocked retention wedge",
        );
}

/// STEER INTO A NON-RESIDENT MEMBER (production 0.8.18 field-acceptance shape,
/// 2026-08-06): a console steer whose arrival TRIGGERS the member build
/// persisted its interaction id into the first run's input row with an
/// EMPTY body - identity survived the whole chain, content did not, and
/// the marker never appeared in the session. This pins meta's half of
/// that chain: a Steer-mode tracked send into a not-yet-materialized
/// member must deliver its body into the first run. Green here means the
/// console hand-off and the identity-first materialization lane carry
/// steer content intact and the residual field loss lives beyond the
/// bridge admission.
#[tokio::test(flavor = "multi_thread")]
async fn steer_into_non_resident_member_delivers_content() {
    const MARKER: &str = "STEER-CONTENT-MARKER-15-XRAY";
    if proxied_to_memo_free_child("steer_into_non_resident_member_delivers_content") {
        return;
    }
    let _serial = SERIAL_WINDOW.lock().await;
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let member = id(MEMBER);

    // Boot 1: one turn so the member has a durable session, then shut down
    // so the next boot starts with the member NON-RESIDENT.
    {
        let capture = CaptureClient::default();
        let runtime = boot(
            &state,
            capture.clone(),
            IdentityBootstrapMode::LazyMaterialize,
        )
        .await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();
        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text("seed turn".to_string()),
            )
            .await
            .expect("seed turn");
        wait_for_turn(&capture, 1, "the seed turn").await;
        runtime.shutdown().await;
    }

    // Boot 2: the very first contact is a STEER-mode tracked send - the
    // steer's own arrival drives materialization (the production shape).
    {
        let capture = CaptureClient::default();
        let runtime = boot(
            &state,
            capture.clone(),
            IdentityBootstrapMode::LazyMaterialize,
        )
        .await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();
        identity_runtime
            .send_with_mode_and_interaction_tracked(
                &member,
                &meerkat_core::ContentInput::Text(format!(
                    "{MARKER}: if you can read this line, reply with the marker."
                )),
                meerkat_core::types::HandlingMode::Steer,
                Some("0818aaaa-1247-5ce3-85c8-17ec20e09a77"),
            )
            .await
            .expect("the steer into a non-resident member must be admitted");
        wait_for_turn(&capture, 1, "the steer-driven first turn").await;
        let last = capture.last().expect("a request was captured");
        assert!(
            last.contains(MARKER),
            "the steer body must reach the first run's request - the field shape lost it \
             (identity persisted, content empty); request: {last}"
        );
        runtime.shutdown().await;
    }
}

/// Profile-prompt revival hygiene (0.8.19 pairing prep, 2026-08-06): a
/// member whose PROFILE declares a prompt through an inline skill, booted
/// repeatedly with a turn each - automatic rematerialization must author
/// NOTHING beyond each turn's own messages, and every boot's request must
/// carry the prompt exactly once (zero copies is a lost prompt, more is
/// revival re-authoring; the durable row count below is the same fact seen
/// from the store). MEASURED FINDING: this is ALREADY GREEN on the
/// 0.8.18 pairing - the identity-first lazy revival and the cold-mint
/// recovery lane both take PreservePersisted paths (host_materialize) and
/// do not mint. The field defect (one assembled row per
/// boot, hundreds of copies) therefore lives in resume paths this harness does not
/// traverse (the mob actor resume AppendExplicit branches, reached through
/// the gateway's bridge session lanes). This test pins the clean lanes as
/// STAYING clean across the 0.8.19 contract change (resumed builds force
/// Inherit); the minting lane needs its own regression at the call site
/// an inspection of the minting lane confirms.
#[tokio::test(flavor = "multi_thread")]
async fn profile_prompt_revival_authors_no_system_rows() {
    if proxied_to_memo_free_child("profile_prompt_revival_authors_no_system_rows") {
        return;
    }
    let _serial = SERIAL_WINDOW.lock().await;
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let member = id(MEMBER);
    let definition = || {
        MobDefinition::from_toml(PROFILE_PROMPT_MOB_TOML)
            .expect("parse profile-prompt mob definition")
    };

    // Boot 1: original spawn (the assembled prompt lands in persisted
    // spawn state here) plus one turn.
    let session_id;
    let mut durable_count;
    {
        let capture = CaptureClient::default();
        let runtime = boot_with_definition_no_customizer(
            &state,
            capture.clone(),
            IdentityBootstrapMode::LazyMaterialize,
            definition(),
        )
        .await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();
        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text("boot 1 turn".to_string()),
            )
            .await
            .expect("boot 1's turn");
        wait_for_turn(&capture, 1, "boot 1's turn").await;
        // The carrier must be real: the profile's inline-skill prompt reaches
        // the freshly spawned member's first request. Without this, the
        // revival assertions below pass for a member with no prompt to
        // re-author.
        assert!(
            PROFILE_PROMPT_MOB_TOML.contains(PROFILE_PROMPT_MARKER),
            "fixture and marker drifted apart"
        );
        let first = capture.last().expect("a boot-1 request was captured");
        assert_eq!(
            first.matches(PROFILE_PROMPT_MARKER).count(),
            1,
            "the profile's inline-skill prompt must reach the spawned member's first \
             request exactly once; request: {first}"
        );
        runtime.shutdown().await;
        let (head, count) = wait_for_durable_document_at_least(&state, 2, "boot 1's commit").await;
        session_id = head;
        durable_count = count;
    }

    // Boots 2 and 3: pure revivals. Any durable growth beyond the turn's
    // own two messages is revival-authored configuration - the defect.
    for boot_no in 2i64..=3 {
        let capture = CaptureClient::default();
        let runtime = boot_with_definition_no_customizer(
            &state,
            capture.clone(),
            IdentityBootstrapMode::LazyMaterialize,
            definition(),
        )
        .await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();
        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text(format!("boot {boot_no} turn")),
            )
            .await
            .unwrap_or_else(|e| panic!("boot {boot_no}'s turn must not be refused: {e}"));
        wait_for_turn(&capture, 1, "a revival boot's turn").await;
        let revived = capture.last().expect("a revival boot request was captured");
        assert_eq!(
            revived.matches(PROFILE_PROMPT_MARKER).count(),
            1,
            "boot {boot_no} must carry the profile prompt exactly once; request: {revived}"
        );
        wait_for_durable_document_at_least(
            &state,
            durable_count + 2,
            "a revival boot's boundary commit",
        )
        .await;
        runtime.shutdown().await;
        let (head, count) =
            wait_for_durable_document_at_least(&state, 0, "the quiescent post-boot document").await;
        assert_eq!(head, session_id, "revivals must extend the same session");
        assert_eq!(
            count,
            durable_count + 2,
            "boot {boot_no} authored {} extra durable row(s): automatic rematerialization \
             re-authored persisted prompt configuration as fresh System input",
            count - durable_count - 2
        );
        durable_count = count;
    }

    // A runtime-store reset is no longer a recovery lane: with the runtime
    // store gone while Core's mob still binds the identity, the next start is
    // refused typed, and restoring the runtime store is the remedy.
    let mut removed = 0;
    for suffix in ["", "-wal", "-shm", ".mfence"] {
        let path = state.join(format!(
            "{}{suffix}",
            meerkat_meta::storage_layout::RUNTIME_DB_FILE_NAME
        ));
        if path.exists() {
            std::fs::remove_file(&path).expect("reset runtime store");
            removed += 1;
        }
    }
    assert!(
        removed > 0,
        "the persistent shape must have written runtime.sqlite"
    );
    let refused = UnifiedRuntimeBuilder::default()
        .definition(definition())
        .persistent_state(&state)
        .continuity_from_state_dir(&state)
        .await
        .err()
        .expect("a start without the runtime store that binds the identity is refused");
    assert!(
        matches!(
            &refused,
            meerkat_meta::UnifiedRuntimeBuilderError::StorageLayout(
                meerkat_meta::storage_layout::StorageLayoutError::RuntimeStoreMissing {
                    bound_identities: 1,
                    ..
                }
            )
        ),
        "{refused:?}"
    );
}

/// Destroy-deprojection regression (2026-07-31), on Core: deleting an
/// identity starts it a fresh life, and a COLD boot after the delete must not
/// resume or otherwise resurrect the deleted transcript: the next send runs on
/// a session other than the deleted one, and its request does not replay the
/// deleted turn.
#[tokio::test(flavor = "multi_thread")]
async fn deleted_identity_starts_a_fresh_life_and_a_cold_boot_does_not_resurrect() {
    const TOKEN: &str = "MARKER-DELETED-RESURRECTION-7-TANGO";
    if proxied_to_memo_free_child(
        "deleted_identity_starts_a_fresh_life_and_a_cold_boot_does_not_resurrect",
    ) {
        return;
    }
    let _serial = SERIAL_WINDOW.lock().await;
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let member = id(MEMBER);

    // --- Boot 1 (scratch runtime): one turn carrying the marker, then
    // DELETE the identity through the public flow. ---
    let deleted_session_id;
    {
        let capture = CaptureClient::default();
        let runtime = boot_lazy_runtime(&state, capture.clone()).await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();
        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text(format!("Please note this token: {TOKEN}")),
            )
            .await
            .expect("send boot 1's turn");
        wait_for_turn(&capture, 1, "boot 1's turn").await;
        wait_for_durable_document_at_least(&state, 2, "boot 1's turn to commit").await;
        deleted_session_id = identity_runtime
            .status(&member)
            .await
            .expect("status after the turn")
            .session_id
            .expect("an active identity owns a durable session");
        identity_runtime
            .delete_identity(&member)
            .await
            .expect("delete the identity");
        runtime.shutdown().await;
    }

    // --- Cold pod: nothing to mint or resume. The first send fresh-spawns
    // under a NEW session id and its request must NOT replay the deleted
    // transcript. ---
    {
        let capture = CaptureClient::default();
        let runtime = boot_lazy_runtime(&state, capture.clone()).await;
        let identity_runtime = runtime
            .identity_runtime()
            .expect("identity runtime")
            .clone();
        identity_runtime
            .send(
                &member,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("the post-delete cold send must fresh-spawn, not wedge or resurrect");
        wait_for_turn(&capture, 1, "the post-delete turn").await;
        let last = capture.last().expect("a post-delete request was captured");
        assert!(
            !last.contains(TOKEN),
            "a deleted transcript must not resurrect into the model's context; request: {last}"
        );
        let after = identity_runtime
            .status(&member)
            .await
            .expect("status after the post-delete send");
        assert_ne!(
            after.session_id.as_ref().map(ToString::to_string),
            Some(deleted_session_id.to_string()),
            "the cold boot must fresh-spawn a NEW session, not resume the deleted id; \
             status: {after:?}"
        );
        runtime.shutdown().await;
    }
}
