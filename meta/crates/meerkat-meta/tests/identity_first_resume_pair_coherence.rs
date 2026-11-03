//! Production cutover regression (2026-07-29): model and provider are a COHERENT
//! PAIR, never independently masked on resume.
//!
//! Incident shape: an explicit operator resume override moved a profile's
//! `model` to another provider's catalog entry with no `provider` key declared.
//! Applying only the model while the durable provider survived minted an invalid
//! pair — (claude-fable-5, openai). Required behavior: an explicit model/provider
//! mask is completed and applied atomically; without an explicit mask durable
//! truth wins whole.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{HandlingMode, StopReason};
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, TopologyProvider};
use meerkat_meta::identity_first::orchestrator::{RestoreOutcome, restore_flow};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, ContinuityStore,
    CustomizerError, DurabilityPolicy, DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider, ManagedPeerEdge, SessionBridge, TopologyContext,
    TopologyError,
};
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;
use meerkat_mob::{MobDefinition, ProfileName};

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22.
#[path = "support/llm_usage.rs"]
mod llm_usage;

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

fn spec(name: &str, profile: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: id(name),
        profile: ProfileName::from(profile),
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

struct EmptyTopology;
#[async_trait]
impl TopologyProvider for EmptyTopology {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        Ok(vec![])
    }
}

struct NoopCustomizer;
#[async_trait]
impl AgentCustomizer for NoopCustomizer {
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
        _session_id: &meerkat_core::types::SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

/// Records the serialized LLM request each turn and answers "ok" so turns
/// complete without a real provider (the injected client serves every
/// provider; the pair under test lives in the build config, not the client).
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
}
impl CaptureClient {
    /// Snapshot of every captured request, in arrival order.
    ///
    /// A turn can produce more than one request, and unrelated turns interleave,
    /// so `last()` is not a reliable way to name "the turn under test" - see
    /// [`select_request_containing`].
    fn all(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// Select the single captured request carrying ALL of `markers`.
///
/// Assertions about a specific turn must run against a request identified by
/// its CONTENT, never by position. `capture.last()` was doing the latter, and
/// on the authored turn it returned whichever request happened to arrive last:
/// the authored one on some runs and an unrelated 2-message replay on others.
/// The visible symptom was a failure that moved between two assertions on the
/// same commit pair, which reads as flakiness in the product rather than in
/// the selector.
///
/// On no match this dumps a summary of every captured request - markers, message
/// count, roles - because "which requests DID arrive" is the first question
/// anyone asks next, and a bare "not found" throws that away.
fn select_request_containing(capture: &CaptureClient, markers: &[(&str, &str)]) -> String {
    // SYNCHRONOUS, and deliberately so. Every delivery in this file now returns
    // only after its OWN turn has committed, so the request under test is
    // already captured by the time we look. There is nothing left to wait for,
    // and the 100ms polling loop this replaces could only ever hide a bug: it
    // turned "the request never arrived" into "the request arrived late",
    // which are different failures with the same symptom.
    //
    // Content-identified, not count-identified and not last-identified: the
    // authored delivery is preceded by an unrelated 2-message replay, so
    // "one more request than before" and "the newest request" both name the
    // decoy.
    let all = capture.all();
    if let Some(one) = all
        .iter()
        .find(|req| markers.iter().all(|(_, needle)| req.contains(needle)))
    {
        return one.clone();
    }
    let all = capture.all();
    let mut summary = String::new();
    for (i, req) in all.iter().enumerate() {
        let parsed: serde_json::Value =
            serde_json::from_str(req).unwrap_or(serde_json::Value::Null);
        let msgs = parsed.get("messages").and_then(|m| m.as_array());
        let present: Vec<&str> = markers
            .iter()
            .filter(|(_, needle)| req.contains(needle))
            .map(|(label, _)| *label)
            .collect();
        summary.push_str(&format!(
            "\n  [{i}] markers_present={present:?} messages={:?} roles={:?}",
            msgs.map(std::vec::Vec::len),
            msgs.map(|m| m
                .iter()
                .map(|x| x.get("role").and_then(|r| r.as_str()).unwrap_or("?"))
                .collect::<Vec<_>>())
        ));
    }
    panic!(
        "no captured request carries all of {:?}; {} request(s) captured:{summary}",
        markers.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
        all.len()
    );
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
        // meerkat 0.8.22 fails a turn closed when its stream carried no
        // normalized provider accounting. A double that hand-rolls `Done`
        // still COMPILES, then fails every turn it drives with
        // `IncompleteResponse` - which surfaces as a member that retries
        // forever rather than as anything resembling a usage error. This file
        // was missed when the other doubles adopted the helper.
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

fn definition(mob_id: &str, model_lines: &str) -> MobDefinition {
    let toml = format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.personal]\n{model_lines}\nexternal_addressable = true\nruntime_mode = \"turn_driven\"\n\n[profiles.personal.tools]\ncomms = true\n"
    );
    MobDefinition::from_toml(&toml).expect("parse pair-coherence mob definition")
}

/// Declare `definition` as the stored definition's successor at
/// `expected_revision` through the spec-update ceremony, the sanctioned path
/// for an edited definition: a persistent mob pins its definition, so a boot
/// carrying an undeclared edit is refused as divergent.
async fn declare_definition(
    state_path: &std::path::Path,
    definition: MobDefinition,
    expected_revision: u64,
) {
    let storage = meerkat_mob::MobStorage::persistent(state_path.join("event_log.sqlite3"))
        .expect("open the mob store");
    let mob_id = definition.id.as_str().to_string();
    let receipt = meerkat_meta::spec_update_ceremony::declare_spec_update(
        &storage,
        &mob_id,
        &definition,
        expected_revision,
    )
    .await
    .expect("declare the edited definition");
    assert_eq!(
        receipt.committed_revision,
        expected_revision + 1,
        "{receipt:?}"
    );
}

/// Build a fresh runtime + identity runtime against `state_path` with the
/// given definition — one process (re)start against a durable store.
async fn boot(
    state_path: &std::path::Path,
    capture: CaptureClient,
    definition: MobDefinition,
    runtime_instance: &str,
) -> (meerkat_meta::UnifiedRuntime, IdentityRuntime) {
    let unified = UnifiedRuntimeBuilder::default()
        .definition(definition)
        .persistent_state(state_path)
        .comms(true)
        .default_llm_client(Arc::new(capture))
        .build()
        .await
        .expect("build UnifiedRuntime");
    let bridge: Arc<dyn SessionBridge> = unified
        .session_bridge()
        .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
        .expect("session_bridge should exist");
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db")).expect("continuity store"),
    );
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store as Arc<dyn ContinuityStore>,
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: runtime_instance.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    });
    (unified, identity_rt)
}

/// THE incident shape, end to end: boot 1 creates the member on
/// `gpt-5.5` (durable provider: openai). Boot 2's definition moves the
/// profile to `claude-opus-4-8` with an explicit model/provider resume mask
/// and NO provider key. The resume must apply the pair atomically — provider
/// derived from the catalog — and succeed onto the same durable session with
/// the transcript intact. Before the pair-coherence fix this resume was REJECTED typed
/// ("model 'claude-opus-4-8' is registered for provider 'anthropic', not
/// provider 'openai'") because only the model was applied.
#[tokio::test(flavor = "multi_thread")]
async fn explicit_model_override_resumes_with_catalog_derived_pair() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice", "personal")];
    const TOKEN: &str = "MARKER-PAIR-3-OSCAR";

    // --- Boot 1: create on gpt-5.5, deliver turn 1, shut down ---
    let original_session_id;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            definition("pair-coherence", "model = \"gpt-5.5\""),
            "pair-coherence-rt",
        )
        .await;

        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Created { binding, .. } => {
                original_session_id = binding
                    .session_id
                    .clone()
                    .expect("Core binds the member to a session");
            }
            other => panic!("expected Created on first boot, got {other:?}"),
        }

        // Subscribe BEFORE the send so the turn's terminal cannot be missed,
        // then await it. The 500ms sleep this replaces was waiting for the turn
        // to COMMIT, which a timer cannot express: it elapses whether the turn
        // succeeded, failed closed, or never ran.
        identity_rt
            .send_awaiting_commit(
                &alice,
                &meerkat_core::ContentInput::Text(format!("Please note this token: {TOKEN}")),
            )
            .await
            .expect("send turn 1");
        unified.shutdown().await;
    }

    // --- Boot 2: SAME store, definition edited to a model owned by another
    // provider, with no provider key but explicit operator resume authority.
    // The pair must move together. ---
    declare_definition(
        &state_path,
        definition(
            "pair-coherence",
            "model = \"claude-opus-4-8\"\nresume_overrides = [\"model\", \"provider\"]",
        ),
        1,
    )
    .await;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            definition(
                "pair-coherence",
                "model = \"claude-opus-4-8\"\nresume_overrides = [\"model\", \"provider\"]",
            ),
            "pair-coherence-rt",
        )
        .await;

        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 2)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "the cross-provider model edit must resume the SAME durable session"
                );
            }
            other => panic!(
                "an explicit model override must RESUME with the catalog-derived pair \
                 (the production rejected-resume regression), got: {other:?}"
            ),
        }

        // Subscribe BEFORE the send so this turn's terminal cannot be missed.
        identity_rt
            .send_awaiting_commit(
                &alice,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send turn 2");
        let last_request = select_request_containing(
            &capture,
            &[("post-edit model", "\"model\":\"claude-opus-4-8\"")],
        );
        assert!(
            last_request.contains("\"model\":\"claude-opus-4-8\""),
            "the resumed turn must run on the DECLARED model (the edit was inert): {last_request}"
        );
        assert!(
            last_request.contains(TOKEN),
            "the resumed turn must replay the persisted transcript (token {TOKEN})"
        );

        // Let turn 2 persist the effective identity before reading it back.
        unified.shutdown().await;
    }

    // --- The durable pair must have moved ATOMICALLY: model and provider
    // both persisted, coherent with the catalog. ---
    {
        use meerkat::SessionStore as _;
        // Core's session store shares runtime.sqlite with its runtime store.
        let db_path = state_path.join("runtime.sqlite");
        let store = meerkat_store::SqliteSessionStore::open(db_path)
            .expect("open the session store for the pair audit");
        let session = store
            .load(&original_session_id)
            .await
            .expect("load resumed session")
            .expect("resumed session exists");
        let metadata = session
            .session_metadata()
            .expect("resumed session carries session metadata");
        assert_eq!(
            metadata.model, "claude-opus-4-8",
            "durable metadata must carry the applied model"
        );
        assert_eq!(
            metadata.provider,
            meerkat_core::Provider::Anthropic,
            "durable metadata must carry the model's catalog owner — the pair moved together \
             or not at all"
        );
    }
}

/// Captures the process's tracing output so the test can assert on emitted
/// log lines. Installed as the global default: the runtime logs from tokio
/// worker threads, which a thread-local subscriber would miss.
#[derive(Clone, Default)]
struct LogCapture {
    buffer: Arc<std::sync::Mutex<Vec<u8>>>,
}
impl LogCapture {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).into_owned()
    }
}
impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

const DIVERGENCE_LINE: &str = "resume restored an LLM identity";

/// When NO coherent (model, provider) pair is resolvable from the
/// declaration (a model the catalog does not know, no `provider` key, no
/// `[models.<id>]` entry), NOTHING may be half-applied and durable truth
/// must survive untouched.
///
/// Under meerkat 0.8.11 definition validation this state is refused at BOOT,
/// typed (`UnknownModel`, naming the profile and model) — a definition that
/// cannot resolve a coherent pair never reaches the resume path at all, so
/// the quiet unmasked-divergence state this test originally exercised
/// (durable-wins-whole resume + once-per-boot INFO tripwire) is
/// unrepresentable end to end for inline profiles: every load-valid inline
/// declaration is auto-marked as a coherent masked pair, and every
/// unresolvable one refuses to boot. The loud typed refusal replaced the
/// quiet INFO line as the tripwire. The pure divergence seam
/// (`unmasked_resume_divergence`) stays unit-covered in
/// `identity_first::bridge`.
///
/// This test encodes the surviving contract: the refused boot leaves the
/// durable store untouched (no half-applied pair, no lost transcript), the
/// refusal names the unresolvable declaration, and the next boot on a
/// resolvable definition resumes the SAME durable session with the
/// transcript intact — and no divergence line fires for that coherent,
/// masked resume.
#[tokio::test(flavor = "multi_thread")]
async fn unresolvable_model_edit_refuses_boot_and_durable_truth_survives() {
    let logs = LogCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(logs.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("this test owns the process's global tracing subscriber");

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:bob", "personal")];
    const TOKEN: &str = "MARKER-PAIR-7-VICTOR";

    // --- Boot 1: create on gpt-5.5, deliver a turn, shut down ---
    let original_session_id;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            definition("pair-divergence", "model = \"gpt-5.5\""),
            "pair-divergence-rt",
        )
        .await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        match result.outcomes.get(&bob).expect("bob outcome") {
            RestoreOutcome::Created { binding, .. } => {
                original_session_id = binding
                    .session_id
                    .clone()
                    .expect("Core binds the member to a session");
            }
            other => panic!("expected Created on first boot, got {other:?}"),
        }
        // Subscribe BEFORE the send so the turn's terminal cannot be missed,
        // then await it. The 500ms sleep this replaces was waiting for the turn
        // to COMMIT, which a timer cannot express: it elapses whether the turn
        // succeeded, failed closed, or never ran.
        identity_rt
            .send_awaiting_commit(
                &bob,
                &meerkat_core::ContentInput::Text(format!("Please note this token: {TOKEN}")),
            )
            .await
            .expect("send turn 1");
        unified.shutdown().await;
    }

    // --- Boot 2: the profile now declares a model NOBODY can resolve a
    // provider for. A persistent mob pins its definition: a boot carrying the
    // undeclared edit is refused as divergent before anything is applied, and
    // declaring the edit is refused typed, naming the unresolvable
    // declaration. Neither half-applies it over the durable identity. ---
    let unresolvable = || definition("pair-divergence", "model = \"model-nobody-knows\"");
    {
        let capture = CaptureClient::default();
        let error = UnifiedRuntimeBuilder::default()
            .definition(unresolvable())
            .persistent_state(&state_path)
            .comms(true)
            .default_llm_client(Arc::new(capture))
            .build()
            .await
            .err()
            .expect("a boot carrying an undeclared definition edit must be refused");
        let rendered = format!("{error:?}");
        for needle in ["Divergent", "profiles.personal.model"] {
            assert!(
                rendered.contains(needle),
                "the boot refusal must name the divergent declaration \
                 (missing `{needle}`): {rendered}"
            );
        }
    }
    {
        let storage = meerkat_mob::MobStorage::persistent(state_path.join("event_log.sqlite3"))
            .expect("open the mob store");
        let error = meerkat_meta::spec_update_ceremony::declare_spec_update(
            &storage,
            "pair-divergence",
            &unresolvable(),
            1,
        )
        .await
        .expect_err("declaring a model no provider resolves must be refused");
        let rendered = format!("{error:?}");
        for needle in ["UnknownModel", "model-nobody-knows", "profiles.personal"] {
            assert!(
                rendered.contains(needle),
                "the declaration refusal must name the unresolvable declaration \
                 (missing `{needle}`): {rendered}"
            );
        }
    }

    // --- Boot 3: back on a resolvable definition. The refused boot must
    // have left durable truth untouched: the SAME session resumes, the
    // transcript replays, the turn runs on the durable pair, and the
    // coherent masked resume fires no divergence line. ---
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            definition("pair-divergence", "model = \"gpt-5.5\""),
            "pair-divergence-rt",
        )
        .await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 3)");
        match result.outcomes.get(&bob).expect("bob outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "the refused boot must not have touched the durable session binding"
                );
            }
            other => panic!(
                "after a refused boot, a resolvable definition must resume the preserved \
                 durable session, got: {other:?}"
            ),
        }

        // Subscribe BEFORE the send so this turn's terminal cannot be missed.
        identity_rt
            .send_awaiting_commit(
                &bob,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send post-refusal turn");
        let last_request = select_request_containing(&capture, &[("TOKEN", TOKEN)]);
        assert!(
            last_request.contains("\"model\":\"gpt-5.5\""),
            "the resumed turn must run on the durable pair: {last_request}"
        );
        assert!(
            last_request.contains(TOKEN),
            "the resumed turn must replay the preserved transcript (token {TOKEN})"
        );

        unified.shutdown().await;
    }

    assert!(
        !logs.contents().contains(DIVERGENCE_LINE),
        "a coherent masked resume of an untouched durable identity must not fire the \
         divergence tripwire"
    );
}

/// Store-level probe for the prompt-update acceptance: the durable session's
/// System-message contents (in transcript order) and its retained
/// transcript-rewrite-commit revisions. The System contents carry the
/// +0/+1 append-counting invariants; the rewrite revisions pin that prompt
/// updates mint NO rewrite commits (the mint class behind the 60-100×
/// revision-bloat incident stays dead).
async fn persisted_prompt_probe(
    state_path: &std::path::Path,
    session_id: &meerkat_core::types::SessionId,
) -> (Vec<String>, Vec<String>) {
    use meerkat::SessionStore as _;
    // Core's session store shares runtime.sqlite with its runtime store.
    let db_path = state_path.join("runtime.sqlite");
    let store = meerkat_store::SqliteSessionStore::open(db_path)
        .expect("open the session store for the prompt probe");
    let session = store
        .load(session_id)
        .await
        .expect("load the durable session")
        .expect("the durable session exists");
    let system_contents = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            meerkat_core::Message::System(system) => Some(system.content.clone()),
            _ => None,
        })
        .collect();
    let rewrite_revisions = session
        .transcript_history_state()
        .expect("decode transcript history state")
        .map(|state| {
            state
                .commits()
                .map(|commit| commit.revision.clone())
                .collect()
        })
        .unwrap_or_default();
    (system_contents, rewrite_revisions)
}

/// Count the prompt-bearing System messages (those carrying the assembled
/// base) — robust against runtime system-context appends, which either
/// mutate an existing System message in place (today) or carry only rendered
/// context without the base (either way, this count moves ONLY on a genuine
/// prompt update).
fn prompt_bearing(system_contents: &[String], base_marker: &str) -> usize {
    system_contents
        .iter()
        .filter(|content| content.contains(base_marker))
        .count()
}

/// Gateway-composed guard for the FINAL meerkat 0.8.11 prompt contract
/// (2026-07-29 ruling): RESUME AUTHORS NOTHING. `Message::System` is an
/// ordinary ordered authored transcript message; prompt policy materializes
/// only when creating an empty transcript, and the only mid-thread
/// instruction change is a caller EXPLICITLY authoring a System message via
/// `StartTurnRequest.system_messages` — exactly once, as part of a turn.
/// +0-on-neutral-resume is STRUCTURAL (no append command exists on the
/// resume path), never an unchanged-value comparison.
///
/// Consequences pinned here, per incident lineage: definition/profile prompt
/// edits are INERT for existing members BY DESIGN (they apply to new
/// members; the released stack's silent inertness becomes
/// the specified behavior instead of a trap); no resume can mint transcript
/// rewrite commits for prompts (the 60-100× revision-bloat class stays
/// dead); no resume can refuse over retained rewrite history for prompt
/// reasons (the mass-Broken class is structurally unreachable);
/// and every ordered System survives resume byte-for-byte (token replay).
///
/// The authored-System leg (+1 exactly once via an explicit turn) is proven
/// by `authored_system_turn_appends_exactly_once_and_replays` below.
#[tokio::test(flavor = "multi_thread")]
async fn resume_never_authors_prompts_and_definition_edits_are_inert_by_design() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let alice = id("personal:alice");
    const TOKEN: &str = "MARKER-PROMPT-EDIT-A-THEN-B";
    const PARAGRAPH_A: &str = "Source-index collections available: alpha.";

    // The definition-owned assembled role instructions (an inline profile
    // skill) - the surface an application edits. Under the final contract this
    // edit must change NOTHING for the existing member.
    let definition_with = |role_instructions: &str| {
        let toml = format!(
            "[mob]\nid = \"prompt-edit\"\n\n[profiles.personal]\nmodel = \"gpt-5.5\"\n\
             skills = [\"role\"]\nexternal_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
             [profiles.personal.tools]\ncomms = true\n\n\
             [skills.role]\nsource = \"inline\"\ncontent = \"{role_instructions}\"\n"
        );
        MobDefinition::from_toml(&toml).expect("parse prompt-edit mob definition")
    };
    const ROLE_BASE: &str = "You are alice, the personal assistant.";
    let roster = vec![spec("personal:alice", "personal")];

    // --- Boot 1: create the member, run a turn, shut down. ---
    let original_session_id;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            definition_with(ROLE_BASE),
            "prompt-edit-rt",
        )
        .await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Created { binding, .. } => {
                original_session_id = binding
                    .session_id
                    .clone()
                    .expect("Core binds the member to a session");
            }
            other => panic!("expected Created on first boot, got {other:?}"),
        }
        // Subscribe BEFORE the send so the turn's terminal cannot be missed,
        // then await it. The 500ms sleep this replaces was waiting for the turn
        // to COMMIT, which a timer cannot express: it elapses whether the turn
        // succeeded, failed closed, or never ran.
        identity_rt
            .send_awaiting_commit(
                &alice,
                &meerkat_core::ContentInput::Text(format!("Please note this token: {TOKEN}")),
            )
            .await
            .expect("send turn 1");
        unified.shutdown().await;
    }

    let (systems_after_create, rewrites_baseline) =
        persisted_prompt_probe(&state_path, &original_session_id).await;
    let base_count = prompt_bearing(&systems_after_create, ROLE_BASE);
    assert!(
        base_count >= 1,
        "the created member must carry the assembled base prompt: {systems_after_create:?}"
    );

    // --- Boot 2: definition prompt edit - must be INERT for the member. ---
    declare_definition(
        &state_path,
        definition_with(&format!("{ROLE_BASE} {PARAGRAPH_A}")),
        1,
    )
    .await;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            definition_with(&format!("{ROLE_BASE} {PARAGRAPH_A}")),
            "prompt-edit-rt",
        )
        .await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 2, edited definition)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "an edited definition must still resume the SAME durable session"
                );
            }
            other => panic!("an edited definition must resume, got {other:?}"),
        }
        // Subscribe BEFORE the send so this turn's terminal cannot be missed.
        identity_rt
            .send_awaiting_commit(
                &alice,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send post-edit turn");
        let last_request = select_request_containing(&capture, &[("TOKEN", TOKEN)]);
        assert!(
            last_request.contains(ROLE_BASE),
            "the durable authored prompt must survive resume byte-for-byte: {last_request}"
        );
        assert!(
            !last_request.contains(PARAGRAPH_A),
            "resume must NOT author the edited definition into the transcript or the \
             live request (resume authors nothing): {last_request}"
        );
        assert!(
            last_request.contains(TOKEN),
            "the resumed turn must replay the persisted transcript (token {TOKEN})"
        );
        unified.shutdown().await;
    }

    let (systems_after_edit, rewrites_after_edit) =
        persisted_prompt_probe(&state_path, &original_session_id).await;
    assert_eq!(
        prompt_bearing(&systems_after_edit, ROLE_BASE),
        base_count,
        "a definition edit must append ZERO System messages on resume: {systems_after_edit:?}"
    );
    assert_eq!(
        prompt_bearing(&systems_after_edit, PARAGRAPH_A),
        0,
        "the edited definition must not reach durable history: {systems_after_edit:?}"
    );
    assert_eq!(
        rewrites_after_edit, rewrites_baseline,
        "no resume may mint transcript rewrite commits for prompts"
    );

    // --- Boot 3: neutral resume — same structural +0. ---
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            definition_with(&format!("{ROLE_BASE} {PARAGRAPH_A}")),
            "prompt-edit-rt",
        )
        .await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (neutral boot 3)");
        match result.outcomes.get(&alice).expect("alice outcome") {
            RestoreOutcome::Resumed { .. } => {}
            other => panic!("a neutral boot must resume, got {other:?}"),
        }
        // Subscribe BEFORE the send so this turn's terminal cannot be missed.
        identity_rt
            .send_awaiting_commit(
                &alice,
                &meerkat_core::ContentInput::Text("neutral ping".to_string()),
            )
            .await
            .expect("send neutral-boot turn");
        select_request_containing(&capture, &[("neutral ping", "neutral ping")]);
        unified.shutdown().await;
    }
    let (systems_final, rewrites_final) =
        persisted_prompt_probe(&state_path, &original_session_id).await;
    assert_eq!(
        prompt_bearing(&systems_final, ROLE_BASE),
        base_count,
        "+0 on neutral resume is structural: {systems_final:?}"
    );
    assert_eq!(
        rewrites_final, rewrites_baseline,
        "neutral resume must not mint transcript rewrite commits"
    );
}

/// Ordered `role:content` projection of the persisted transcript for ordinal
/// assertions - the prompt probe above collapses to System contents only and
/// cannot see position. Roles other than System/User project as an opaque
/// label; the authored-leg ordering contract only needs those two.
async fn persisted_ordered_rows(
    state_path: &std::path::Path,
    session_id: &meerkat_core::types::SessionId,
) -> Vec<String> {
    use meerkat::SessionStore as _;
    // Core's session store shares runtime.sqlite with its runtime store.
    let db_path = state_path.join("runtime.sqlite");
    let store = meerkat_store::SqliteSessionStore::open(db_path)
        .expect("open the session store for the ordinal probe");
    let session = store
        .load(session_id)
        .await
        .expect("load the durable session")
        .expect("the durable session exists");
    session
        .messages()
        .iter()
        .map(|message| match message {
            meerkat_core::Message::System(system) => format!("system:{}", system.content),
            meerkat_core::Message::User(user) => format!("user:{}", user.text_content()),
            _ => "other".to_string(),
        })
        .collect()
}

/// Index of the single transcript row containing `needle`, asserting
/// uniqueness so an ordinal claim can never pass against a duplicated row
/// (a duplicate IS the dedup/merge failure this file exists to catch).
fn sole_index_containing(rows: &[String], needle: &str) -> usize {
    let hits: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| row.contains(needle).then_some(index))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one transcript row containing {needle:?}: {rows:?}"
    );
    hits[0]
}

/// The authored-System leg of the FINAL meerkat 0.8.11 prompt contract
/// (queue item 8b), composed through the mob deliver path itself: an
/// explicit per-turn System instruction rides
/// `SessionBridge::deliver_with_mode_context_and_system_prompt` ->
/// `WorkSpec::system_prompt` (wave011 commit 6eb69017) ->
/// `RuntimeTurnMetadata.system_prompts`, and meerkat appends it as ONE
/// ordinary ordered `Message::System` row at that turn's admitted
/// transcript boundary - after all prior history, before the turn's own
/// user message.
///
/// Pinned here:
/// - the authored turn appends EXACTLY ONE System row (+1 total, +1
///   marker-bearing), ordered between the prior turn's history and its own
///   user message - authorship happens at the turn boundary, never as a
///   hoisted prompt rewrite;
/// - prior System rows are untouched (the assembled base prompt survives at
///   its create-time count - no replacement, no merge) and no transcript
///   rewrite commits are minted (the 60-100x revision-bloat class stays dead
///   for the authored leg too);
/// - the row REPLAYS: after a cold restart/resume it is still present
///   exactly once, at the SAME ordinal, with its full authored content, and
///   it projects into the resumed turn's LLM request (no dedup, no hoist,
///   no merge, no re-authoring on resume);
/// - a subsequent turn WITHOUT an authored System prompt (the ordinary
///   `IdentityRuntime::send` path, which rides the same carrier with `None`)
///   appends ZERO additional System rows.
///
/// Per-wire projection (OpenAI in-place system role, Anthropic encoded
/// placement, per-model typed projection failures) is deliberately out of
/// scope: this harness injects a capture client whose replay projection is
/// the identity, so it proves the persistence + replay contract only.
#[tokio::test(flavor = "multi_thread")]
async fn authored_system_turn_appends_exactly_once_and_replays() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let carol = id("personal:carol");
    let roster = vec![spec("personal:carol", "personal")];
    const TOKEN: &str = "MARKER-AUTHORED-8B-KILO";
    const AUTHORED_MARKER: &str = "MARKER-AUTHORED-SYS-8B";
    const AUTHORED_PROMPT: &str =
        "Standing instruction MARKER-AUTHORED-SYS-8B: address the operator as Commander.";
    const TURN_2_TEXT: &str = "Acknowledge the standing instruction.";
    const ROLE_BASE: &str = "You are carol, the records clerk.";

    let mob_definition = || {
        let toml = format!(
            "[mob]\nid = \"authored-system\"\n\n[profiles.personal]\nmodel = \"gpt-5.5\"\n\
             skills = [\"role\"]\nexternal_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
             [profiles.personal.tools]\ncomms = true\n\n\
             [skills.role]\nsource = \"inline\"\ncontent = \"{ROLE_BASE}\"\n"
        );
        MobDefinition::from_toml(&toml).expect("parse authored-system mob definition")
    };

    // --- Boot 1: create the member, run a plain turn, shut down. ---
    let original_session_id;
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            mob_definition(),
            "authored-system-rt",
        )
        .await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 1)");
        match result.outcomes.get(&carol).expect("carol outcome") {
            RestoreOutcome::Created { binding, .. } => {
                original_session_id = binding
                    .session_id
                    .clone()
                    .expect("Core binds the member to a session");
            }
            other => panic!("expected Created on first boot, got {other:?}"),
        }
        // Subscribe BEFORE the send so the turn's terminal cannot be missed,
        // then await it. The 500ms sleep this replaces was waiting for the turn
        // to COMMIT, which a timer cannot express: it elapses whether the turn
        // succeeded, failed closed, or never ran.
        identity_rt
            .send_awaiting_commit(
                &carol,
                &meerkat_core::ContentInput::Text(format!("Please note this token: {TOKEN}")),
            )
            .await
            .expect("send turn 1");
        unified.shutdown().await;
    }

    let (systems_baseline, rewrites_baseline) =
        persisted_prompt_probe(&state_path, &original_session_id).await;
    let base_count = prompt_bearing(&systems_baseline, ROLE_BASE);
    assert!(
        base_count >= 1,
        "the created member must carry the assembled base prompt: {systems_baseline:?}"
    );
    assert_eq!(
        prompt_bearing(&systems_baseline, AUTHORED_MARKER),
        0,
        "the authored marker must not predate the authored turn: {systems_baseline:?}"
    );
    let total_baseline = systems_baseline.len();

    // --- Boot 2: resume, then author ONE System message through the mob
    // deliver path itself (the WorkSpec.system_prompt carrier). ---
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            mob_definition(),
            "authored-system-rt",
        )
        .await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 2)");
        match result.outcomes.get(&carol).expect("carol outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "boot 2 must resume the SAME durable session"
                );
            }
            other => panic!("expected Resumed on boot 2, got {other:?}"),
        }

        // The authored leg now goes through the IdentityRuntime like every
        // other delivery. It used to drive the session bridge directly, because
        // the send surface had no per-turn System parameter; it does now, and
        // going around the runtime meant this turn alone skipped lease
        // validation, alias pinning and session reconciliation.
        //
        // Returns only after THIS turn commits, so there is no fuse, no
        // interaction-event correlation and no sleep.
        identity_rt
            .send_awaiting_commit_with_system_prompt(
                &carol,
                &meerkat_core::ContentInput::Text(TURN_2_TEXT.to_string()),
                Some(AUTHORED_PROMPT),
                HandlingMode::Queue,
                None,
            )
            .await
            .expect("the authored system turn must complete");
        assert_eq!(
            identity_rt
                .status(&carol)
                .await
                .expect("carol status")
                .session_id
                .expect("carol must still be bound to a session"),
            original_session_id,
            "the authored turn must land on the same durable session"
        );
        // Identify the authored turn by CONTENT - it is the request carrying
        // both the authored System prompt and this turn's user text - and then
        // assert the transcript on THAT SAME request. Selecting by position
        // (`capture.last()`) made the failure move between assertions on one
        // commit pair, because unrelated requests interleave.
        let authored_request = select_request_containing(
            &capture,
            &[
                ("AUTHORED_PROMPT", AUTHORED_PROMPT),
                ("TURN_2_TEXT", TURN_2_TEXT),
            ],
        );
        assert!(
            authored_request.contains(TOKEN),
            "the authored turn must replay the persisted transcript (token {TOKEN}): \
             {authored_request}"
        );
        // No commit sleep: the send returned only after this turn committed.
        unified.shutdown().await;
    }

    let (systems_after_authored, rewrites_after_authored) =
        persisted_prompt_probe(&state_path, &original_session_id).await;
    assert_eq!(
        systems_after_authored.len(),
        total_baseline + 1,
        "the authored turn must append EXACTLY ONE System row: {systems_after_authored:?}"
    );
    assert_eq!(
        prompt_bearing(&systems_after_authored, AUTHORED_MARKER),
        1,
        "exactly one authored System row: {systems_after_authored:?}"
    );
    assert_eq!(
        prompt_bearing(&systems_after_authored, ROLE_BASE),
        base_count,
        "an authored append must not replace or merge prior System rows: \
         {systems_after_authored:?}"
    );
    assert_eq!(
        rewrites_after_authored, rewrites_baseline,
        "authoring a System message is an ordinary turn append - it must mint NO \
         transcript rewrite commits"
    );

    let rows_after_authored = persisted_ordered_rows(&state_path, &original_session_id).await;
    let authored_index = sole_index_containing(&rows_after_authored, AUTHORED_MARKER);
    let turn_1_user_index = sole_index_containing(&rows_after_authored, TOKEN);
    let turn_2_user_index = sole_index_containing(&rows_after_authored, TURN_2_TEXT);
    assert!(
        turn_1_user_index < authored_index,
        "the authored System row belongs at ITS turn's boundary, not hoisted above prior \
         history (turn-1 user at {turn_1_user_index}, authored System at {authored_index}): \
         {rows_after_authored:?}"
    );
    assert!(
        authored_index < turn_2_user_index,
        "the authored System row must be ordered BEFORE its own turn's user message \
         (authored System at {authored_index}, turn-2 user at {turn_2_user_index}): \
         {rows_after_authored:?}"
    );
    assert!(
        rows_after_authored[authored_index].contains(AUTHORED_PROMPT),
        "the authored System row must carry the full authored content: {rows_after_authored:?}"
    );

    // --- Boot 3: cold resume, then a turn WITHOUT an authored System prompt
    // (the ordinary send path - the same carrier, threaded as `None`). ---
    {
        let capture = CaptureClient::default();
        let (unified, identity_rt) = boot(
            &state_path,
            capture.clone(),
            mob_definition(),
            "authored-system-rt",
        )
        .await;
        let result = restore_flow(
            &identity_rt,
            &roster,
            Some(&EmptyTopology as &dyn TopologyProvider),
            Some(&NoopCustomizer as &dyn AgentCustomizer),
        )
        .await
        .expect("restore_flow (boot 3)");
        match result.outcomes.get(&carol).expect("carol outcome") {
            RestoreOutcome::Resumed { binding, .. } => {
                assert_eq!(
                    binding.session_id.as_ref(),
                    Some(&original_session_id),
                    "boot 3 must resume the SAME durable session"
                );
            }
            other => panic!("expected Resumed on boot 3, got {other:?}"),
        }
        // Subscribe BEFORE the send so this turn's terminal cannot be missed.
        identity_rt
            .send_awaiting_commit(
                &carol,
                &meerkat_core::ContentInput::Text("What token did I give you earlier?".to_string()),
            )
            .await
            .expect("send the neutral turn");
        let last_request = select_request_containing(
            &capture,
            &[("neutral turn text", "What token did I give you earlier?")],
        );
        assert!(
            last_request.contains(AUTHORED_PROMPT),
            "the authored System row must replay byte-for-byte into the resumed turn's \
             request: {last_request}"
        );
        assert!(
            last_request.contains(TOKEN),
            "the resumed turn must replay the persisted transcript (token {TOKEN})"
        );
        unified.shutdown().await;
    }

    let (systems_final, rewrites_final) =
        persisted_prompt_probe(&state_path, &original_session_id).await;
    assert_eq!(
        systems_final.len(),
        total_baseline + 1,
        "a resume plus a turn WITHOUT an authored System prompt must append ZERO \
         additional System rows: {systems_final:?}"
    );
    assert_eq!(
        prompt_bearing(&systems_final, AUTHORED_MARKER),
        1,
        "no dedup, no re-authoring: the authored row survives exactly once: {systems_final:?}"
    );
    assert_eq!(
        prompt_bearing(&systems_final, ROLE_BASE),
        base_count,
        "the base prompt rows survive the authored leg untouched: {systems_final:?}"
    );
    assert_eq!(
        rewrites_final, rewrites_baseline,
        "neither the authored turn nor its replay may mint transcript rewrite commits"
    );

    let rows_final = persisted_ordered_rows(&state_path, &original_session_id).await;
    let authored_index_final = sole_index_containing(&rows_final, AUTHORED_MARKER);
    assert_eq!(
        authored_index_final, authored_index,
        "the authored System row must replay at the SAME ordinal (no hoist, no merge, \
         no move): {rows_final:?}"
    );
    assert!(
        rows_final[authored_index_final].contains(AUTHORED_PROMPT),
        "the replayed authored System row must keep its full authored content: {rows_final:?}"
    );
}
