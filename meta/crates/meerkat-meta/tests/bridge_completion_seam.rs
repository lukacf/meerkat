//! Adversarial regressions for the completion-bearing bridge seam.
//!
//! Both cover defects that existed in this seam and were caught in review, not
//! by a test. They are written to fail if either is reintroduced.
//!
//! 1. The completion wait must NOT be charged to the actor admission budget.
//!    That budget is wall clock and also covers session resolution, so awaiting
//!    a turn inside it makes an ordinary long turn look like an
//!    `ActorAdmissionTimeout` on a step that never misbehaved.
//!
//! 2. A turn that was admitted and then FAILED must surface as a typed
//!    `CompletionFailed`, never as an admission-shaped error. The delivery path
//!    retries "repairable" admission failures; if a completion failure reached
//!    that classifier, the member's turn would be resubmitted and RUN A SECOND
//!    TIME. Typing it is what keeps it out of that path.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{HandlingMode, StopReason};
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::bridge::MobSessionBridge;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, TopologyProvider};
use meerkat_meta::identity_first::orchestrator::{RestoreOutcome, restore_flow};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, AgentRuntimeId,
    BridgeError, ContinuityStore, CustomizerError, DurabilityPolicy, DurableAgentSpec,
    IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider,
    ManagedPeerEdge, SessionBridge, TopologyContext, TopologyError,
};
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;
use meerkat_mob::{MobDefinition, ProfileName};

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[path = "support/llm_usage.rs"]
mod llm_usage;

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

fn spec(name: &str) -> DurableAgentSpec {
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

/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn mob_toml() -> String {
    format!(
        r#"
[mob]
id = "bridge-completion-seam-{}"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
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

/// LLM double with a controllable turn duration and conformance.
///
/// `emit_usage = false` reproduces a turn that fails AFTER admission: meerkat
/// 0.8.22 fails a turn closed when its stream carried no normalized provider
/// accounting. That is a real post-admission failure rather than a simulated
/// one, which is what makes it a fair test of the completion path.
/// Sentinel wording deliberately chosen to be ACCEPTED by
/// `is_missing_bridge_session_snapshot_error`, and therefore by
/// `is_repairable_bridge_delivery_error` (`bridge.rs:39,66`: it matches on
/// `contains("missing bridge session snapshot")`).
///
/// This is the whole point of the second regression. A post-admission failure
/// the classifier would IGNORE proves nothing - the retry path would not have
/// fired for it either way. Only a failure the classifier would have SWALLOWED
/// can demonstrate that phase separation, not luck, is what prevents the
/// resubmission.
const REPAIRABLE_SENTINEL: &str = "missing bridge session snapshot - completion sentinel";

#[derive(Clone)]
struct ScriptedClient {
    /// Signalled once the turn has entered the LLM. Lets a test know the work
    /// is genuinely in flight without waiting on a clock.
    entered: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    /// Held until the test releases it. While held, the turn cannot finish, so
    /// "the delivery has not returned" is a fact rather than a race.
    release: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    /// When false the turn fails AFTER admission, carrying REPAIRABLE_SENTINEL.
    emit_usage: bool,
    calls: Arc<AtomicUsize>,
}

impl ScriptedClient {
    fn immediate(emit_usage: bool) -> Self {
        Self {
            entered: Arc::new(tokio::sync::Mutex::new(None)),
            release: Arc::new(tokio::sync::Mutex::new(None)),
            emit_usage,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl meerkat_client::LlmClient for ScriptedClient {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        let entered = Arc::clone(&self.entered);
        let release = Arc::clone(&self.release);
        let terminal = self.emit_usage.then(|| {
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn)
        });
        Box::pin(async_stream::stream! {
            // Announce that the turn is in flight, then block until released.
            // No clocks: the test drives both edges.
            if let Some(tx) = entered.lock().await.take() {
                let _ = tx.send(());
            }
            if let Some(rx) = release.lock().await.take() {
                let _ = rx.await;
            }
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            match terminal {
                Some([usage, done]) => {
                    yield Ok(usage);
                    yield Ok(done);
                }
                // A typed post-admission failure whose message the repairable
                // classifier accepts. The work WAS admitted; the turn then
                // failed. It must be a failure meerkat does NOT retry inside
                // the turn, so that one LLM call proves Meta did not
                // resubmit: meerkat 0.8.51 retries `IncompleteResponse` (a
                // truncated stream) under its bounded agent-loop policy, so
                // that variant would count meerkat's own replays here.
                None => {
                    yield Err(LlmError::StreamParseError {
                        message: REPAIRABLE_SENTINEL.to_string(),
                    });
                }
            }
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

/// Boot a runtime around `client` and return the bridge plus one member's
/// runtime id.
async fn boot_one_member(
    state_path: &std::path::Path,
    client: ScriptedClient,
    admission_budget: Duration,
) -> (
    meerkat_meta::UnifiedRuntime,
    Arc<dyn SessionBridge>,
    AgentRuntimeId,
) {
    let unified = UnifiedRuntimeBuilder::default()
        .definition(MobDefinition::from_toml(&mob_toml()).expect("parse seam mob definition"))
        .persistent_state(state_path)
        .comms(true)
        .default_llm_client(Arc::new(client))
        .build()
        .await
        .expect("build UnifiedRuntime");
    // Build the bridge explicitly so the actor admission budget can be set,
    // exactly as tests/repair_queue_carry.rs:245 does. A tight budget is what
    // makes the boundary observable: the production default is ten minutes.
    let session_service = unified
        .mob_runtime()
        .session_service()
        .cloned()
        .expect("session service");
    let bridge: Arc<dyn SessionBridge> = Arc::new(
        MobSessionBridge::with_session_service(unified.mob_handle(), session_service)
            .with_actor_admission_budget(admission_budget),
    );
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db")).expect("continuity store"),
    );
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store as Arc<dyn ContinuityStore>,
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "bridge-completion-seam".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge.clone()),
        default_timeout: None,
    });

    let alice = id("alice");
    let roster = vec![spec("alice")];
    let result = restore_flow(
        &identity_rt,
        &roster,
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");
    let runtime_id = match result.outcomes.get(&alice).expect("alice outcome") {
        RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. } => {
            binding.agent_runtime_id.clone()
        }
        other => panic!("expected the member to be created, got {other:?}"),
    };
    (unified, bridge, runtime_id)
}

/// The delivery must actually AWAIT the turn - proven by a barrier, not a clock.
///
/// The double signals `entered` once the turn is in flight and then blocks. While
/// it is blocked the spawned delivery MUST NOT have finished: if the completion
/// handle were dropped (the defect review caught here), the delivery would
/// return at admission and the join handle would already be complete. Releasing
/// the barrier then has to produce success.
///
/// No elapsed time is used as evidence anywhere: the test drives both edges.
#[tokio::test(flavor = "multi_thread")]
async fn the_delivery_awaits_the_turn_and_does_not_drop_the_handle() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let client = ScriptedClient {
        entered: Arc::new(tokio::sync::Mutex::new(Some(entered_tx))),
        release: Arc::new(tokio::sync::Mutex::new(Some(release_rx))),
        emit_usage: true,
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let (unified, bridge, runtime_id) = boot_one_member(
        &temp.path().join("state"),
        client.clone(),
        Duration::from_secs(30),
    )
    .await;

    let delivery = tokio::spawn({
        let bridge = Arc::clone(&bridge);
        let runtime_id = runtime_id.clone();
        async move {
            bridge
                .deliver_awaiting_commit_with_mode_context_and_system_prompt(
                    &runtime_id,
                    &meerkat_core::ContentInput::Text("held at the barrier".to_string()),
                    None,
                    &[],
                    HandlingMode::Queue,
                    None,
                )
                .await
        }
    });

    entered_rx.await.expect("the turn must reach the LLM");
    // SMOKE ONLY, deliberately not causal. A spawned task that has not been
    // polled yet is also "not finished", so this cannot PROVE the handle is
    // awaited - it only catches the blatant case. The causal proof of the
    // ordering lives in the direct-poll unit test
    // `session_resolution_completes_before_the_completion_future_is_polled`,
    // which was verified by negative mutation: swapping the awaits makes it
    // fail inside the completion future's first poll.
    assert!(
        !delivery.is_finished(),
        "smoke: the delivery returned while the turn was still held at the barrier"
    );

    release_tx.send(()).expect("release the turn");
    let delivered = delivery.await.expect("delivery task panicked");
    assert!(
        delivered.is_ok(),
        "the delivery must succeed once the turn completes, got {:?}",
        delivered.err()
    );
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        1,
        "the turn must have reached the LLM exactly once"
    );
    unified.shutdown().await;
}

/// A turn that fails AFTER admission must be typed, and must not be retried.
///
/// `CompletionFailed` is what keeps it out of the delivery path's
/// repair-and-retry classifier. If it came back as an admission-shaped error,
/// that classifier could resubmit and run the member's turn twice.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_fails_after_admission_is_typed_and_not_retried() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let client = ScriptedClient::immediate(false);
    let (unified, bridge, runtime_id) = boot_one_member(
        &temp.path().join("state"),
        client.clone(),
        Duration::from_secs(30),
    )
    .await;

    let delivered = bridge
        .deliver_awaiting_commit_with_mode_context_and_system_prompt(
            &runtime_id,
            &meerkat_core::ContentInput::Text("this turn will fail closed".to_string()),
            None,
            &[],
            HandlingMode::Queue,
            None,
        )
        .await;

    match delivered {
        Err(BridgeError::CompletionFailed(detail)) => {
            assert!(
                detail.contains("missing bridge session snapshot"),
                "the injected failure must be one the repairable classifier WOULD accept, \
                 otherwise this test proves nothing about the retry path; got: {detail}"
            );
        }
        other => panic!(
            "a turn that was admitted and then failed must surface as \
             BridgeError::CompletionFailed - anything admission-shaped can reach the \
             repair-and-retry classifier and run the turn twice. Got: {other:?}"
        ),
    }
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        1,
        "the failed turn must NOT have been resubmitted. Its detail is accepted by \
         is_repairable_bridge_delivery_error, so had the completion failure reached that \
         classifier the delivery path would have repaired the member and resubmitted, \
         running the turn a second time. Exactly one call proves phase separation holds."
    );
    unified.shutdown().await;
}

/// `MobRuntime::session_commit_pending` forwards meerkat's
/// `session_has_uncommitted_run_input` on the real runtime machine: `false`
/// once the member's turn is committed, `true` while a turn holds its input
/// in the model, and `false` again after that turn commits.
#[tokio::test(flavor = "multi_thread")]
async fn session_commit_pending_reads_the_members_run_input_phase() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let client = ScriptedClient::immediate(true);
    let (unified, bridge, runtime_id) = boot_one_member(
        &temp.path().join("state"),
        client.clone(),
        Duration::from_secs(30),
    )
    .await;
    let deliver = |text: &'static str| {
        let bridge = Arc::clone(&bridge);
        let runtime_id = runtime_id.clone();
        async move {
            bridge
                .deliver_awaiting_commit_with_mode_context_and_system_prompt(
                    &runtime_id,
                    &meerkat_core::ContentInput::Text(text.to_string()),
                    None,
                    &[],
                    HandlingMode::Queue,
                    None,
                )
                .await
        }
    };
    deliver("first, committed").await.expect("first turn");
    let member = unified
        .mob_handle()
        .list_all_members()
        .await
        .into_iter()
        .next()
        .expect("the booted member")
        .agent_identity;
    let session_id = unified
        .mob_handle()
        .get_member(&member)
        .await
        .expect("read the member")
        .and_then(|entry| entry.bridge_session_id().cloned())
        .expect("the member has a session")
        .to_string();
    let runtime = unified.mob_runtime();

    assert_eq!(
        runtime.session_commit_pending(&session_id).await,
        Some(false),
        "a committed turn leaves no uncommitted run input"
    );

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *client.entered.lock().await = Some(entered_tx);
    *client.release.lock().await = Some(release_rx);
    let busy = tokio::spawn(deliver("second, held"));
    entered_rx.await.expect("the second turn reaches the model");
    assert_eq!(
        runtime.session_commit_pending(&session_id).await,
        Some(true),
        "a turn in the model holds its input uncommitted"
    );

    release_tx.send(()).expect("release the held turn");
    busy.await
        .expect("delivery task")
        .expect("the held turn completes");
    assert_eq!(
        runtime.session_commit_pending(&session_id).await,
        Some(false),
        "the held turn's commit leaves nothing pending"
    );

    unified.shutdown().await;
}

/// `session_commit_pending` fails closed (`None`, reconcile nothing) whenever
/// no live runtime session answers: no runtime machine, a session id that
/// does not parse, a session no runtime ever held, and a session that is
/// gone because its member retired.
#[tokio::test(flavor = "multi_thread")]
async fn session_commit_pending_fails_closed_without_a_live_runtime_session() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let client = ScriptedClient::immediate(true);
    let (unified, bridge, runtime_id) = boot_one_member(
        &temp.path().join("state"),
        client.clone(),
        Duration::from_secs(30),
    )
    .await;
    bridge
        .deliver_awaiting_commit_with_mode_context_and_system_prompt(
            &runtime_id,
            &meerkat_core::ContentInput::Text("committed".to_string()),
            None,
            &[],
            HandlingMode::Queue,
            None,
        )
        .await
        .expect("a committed turn");
    let member = unified
        .mob_handle()
        .list_all_members()
        .await
        .into_iter()
        .next()
        .expect("the booted member")
        .agent_identity;
    let session_id = unified
        .mob_handle()
        .get_member(&member)
        .await
        .expect("read the member")
        .and_then(|entry| entry.bridge_session_id().cloned())
        .expect("the member has a session")
        .to_string();
    let runtime = unified.mob_runtime();
    assert_eq!(
        runtime.session_commit_pending(&session_id).await,
        Some(false),
        "control: the live session answers"
    );

    // No runtime machine: a runtime adopted from a bare handle has none.
    let bare = meerkat_meta::mob_handle_runtime::MobRuntime::from_handle(unified.mob_handle());
    assert_eq!(bare.session_commit_pending(&session_id).await, None);
    // A session id that does not parse.
    assert_eq!(
        runtime.session_commit_pending("not-a-session-id").await,
        None
    );
    // A session no runtime ever held.
    assert_eq!(
        runtime
            .session_commit_pending(&meerkat_core::SessionId::new().to_string())
            .await,
        None
    );
    // A session that is gone: its member retired.
    unified
        .mob_handle()
        .retire(member)
        .await
        .expect("retire the member");
    assert_eq!(
        runtime.session_commit_pending(&session_id).await,
        None,
        "a retired member's session is gone, not quiet"
    );
    unified.shutdown().await;
}

/// A reset's superseded member retires through Core alone when no continuity
/// session copy is composed, as in this file's bridge: there is no Meta
/// projection to abandon, so the retire neither refuses nor leaves the
/// member's roster row behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_superseded_member_retires_through_core_without_a_session_copy() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (unified, bridge, runtime_id) = boot_one_member(
        &temp.path().join("state"),
        ScriptedClient::immediate(true),
        Duration::from_secs(30),
    )
    .await;
    let member = unified
        .mob_handle()
        .list_all_members()
        .await
        .into_iter()
        .next()
        .expect("the booted member")
        .agent_identity;
    let session_id = unified
        .mob_handle()
        .get_member(&member)
        .await
        .expect("read the member")
        .and_then(|entry| entry.bridge_session_id().cloned())
        .expect("the member has a session");

    bridge
        .retire_reset_superseded_member(&runtime_id, &session_id)
        .await
        .expect("with no session copy composed, the retire runs through Core alone");

    assert!(
        unified
            .mob_handle()
            .list_all_members()
            .await
            .iter()
            .all(|entry| entry.agent_identity != member),
        "the superseded member must leave no roster row"
    );
    unified.shutdown().await;
}

/// Core's typed Absent is the bridge's typed refusal, never a fresh spawn of
/// its own: resuming a session Core holds no durable copy of refuses with
/// `Unavailable { reason: Absent }` naming the session, and no member is
/// spawned for the identity. The caller decides what happens next.
#[tokio::test(flavor = "multi_thread")]
async fn an_absent_session_is_refused_typed_and_never_fresh_spawned() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (unified, bridge, _runtime_id) = boot_one_member(
        &temp.path().join("state"),
        ScriptedClient::immediate(true),
        Duration::from_secs(30),
    )
    .await;
    let ghost = spec("ghost");
    let draft = AgentBuildDraft {
        compaction_curator: Default::default(),
        model: None,
        system_prompt: None,
        additional_instructions: ghost.additional_instructions.clone(),
        labels: ghost.labels.clone(),
        app_context: ghost.context.clone(),
        external_tools: Vec::new(),
        local_external_tools: Default::default(),
        provider_params: None,
    };
    let absent = meerkat_core::types::SessionId::new();

    let refusal = bridge
        .resume_session(&ghost.identity, &ghost, &draft, &absent, None)
        .await
        .expect_err("a session Core holds no copy of must be refused, not fresh-spawned");
    let BridgeError::ResumeRejected { kind, .. } = &refusal else {
        panic!("expected a typed resume refusal, got {refusal:?}");
    };
    assert_eq!(format!("{kind:?}"), "Unavailable { reason: Absent }");
    assert!(
        refusal.to_string().contains(&absent.to_string()),
        "the refusal must name the session: {refusal}"
    );
    assert!(
        unified
            .mob_handle()
            .list_all_members()
            .await
            .iter()
            .all(|entry| !entry.agent_identity.as_str().contains("ghost")),
        "no member may be spawned for the refused identity"
    );
    unified.shutdown().await;
}

/// The identity-keyed reads answer from Core. `live_member` and
/// `live_member_scope` report the booted member's committed incarnation and
/// session, `ensure_member_live` reports the same binding, the convergence
/// status of an identity no intent manages reads as missing, and an identity
/// Core holds no member for reads as `None` everywhere.
#[tokio::test(flavor = "multi_thread")]
async fn identity_keyed_reads_answer_from_core() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (unified, bridge, runtime_id) = boot_one_member(
        &temp.path().join("state"),
        ScriptedClient::immediate(true),
        Duration::from_secs(30),
    )
    .await;
    let alice = id("alice");

    let live = bridge
        .live_member(&alice)
        .await
        .expect("read the live member")
        .expect("Core holds the booted member");
    assert_eq!(
        live.agent_runtime_id, runtime_id,
        "the live member is the incarnation the runtime recorded"
    );
    let session_id = live
        .session_id
        .clone()
        .expect("the booted member is session-backed");

    let scope = bridge
        .live_member_scope(&alice)
        .await
        .expect("capture the delivery scope")
        .expect("Core holds the booted member");
    assert_eq!(scope.session_id(), &session_id);

    let ensured = bridge
        .ensure_member_live(&alice)
        .await
        .expect("ensure the member is live")
        .expect("Core holds the booted member");
    assert_eq!(ensured, live);

    assert!(matches!(
        bridge
            .identity_convergence_status(&alice)
            .await
            .expect("read the convergence status"),
        meerkat_mob::identity::IdentityStoredObservation::Missing
    ));

    let ghost = id("ghost");
    assert!(bridge.live_member(&ghost).await.expect("read").is_none());
    assert!(
        bridge
            .live_member_scope(&ghost)
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        bridge
            .ensure_member_live(&ghost)
            .await
            .expect("read")
            .is_none()
    );
    unified.shutdown().await;
}

/// A host-adopted identity is refused typed, and nothing moves. Meta writes
/// no Core identity intent row, so a row Core holds for an identity is the
/// host's: here Core's own `adopt_member_identity_declaration` writes
/// revision 1 naming the member's live session. `ensure_member_live` and the
/// resume collision arm read that row before any other step and refuse
/// `HostAdopted`. The roster entry (generation, fence, runtime id, role,
/// mode, labels, status), the member's session binding and Core's member
/// lifecycle events stay exactly as they were.
///
/// The refusal is matched by its Debug name so this test also compiles, and
/// fails for the right reason, against a bridge that has no such variant.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_adopted_identity_is_refused_typed_and_nothing_moves() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (unified, bridge, runtime_id) = boot_one_member(
        &temp.path().join("state"),
        ScriptedClient::immediate(true),
        Duration::from_secs(30),
    )
    .await;
    // One committed turn, so the adopted session has a durable transcript.
    bridge
        .deliver_awaiting_commit_with_mode_context_and_system_prompt(
            &runtime_id,
            &meerkat_core::ContentInput::Text("before the host adopts".to_string()),
            None,
            &[],
            HandlingMode::Queue,
            None,
        )
        .await
        .expect("a committed turn before adoption");

    let handle = unified.mob_handle();
    let alice = spec("alice");
    let member = meerkat_meta::member_comms_id::mob_member_id(alice.identity.as_str());
    let session_id = handle
        .resolve_bridge_session_id(&member)
        .await
        .expect("the booted member is session-backed");
    let entry = handle
        .get_member(&member)
        .await
        .expect("roster read")
        .expect("Core holds the booted member");

    // The host adopts the identity: revision 1 naming the live session.
    let params: meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams =
        serde_json::from_value(serde_json::json!({
            "mob_id": handle.mob_id().to_string(),
            "agent_identity": member.to_string(),
            "request_id": "host-adopts-alice",
            "precondition": "expected_absent",
            "declaration_scope": "host-adoption-test",
            "declaration_revision": 1,
            "session": {
                "session_id": session_id.to_string(),
                "lineage_id": format!("session:{session_id}"),
                "lineage_generation": 0,
                "authority_policy": "require_existing"
            },
            "member": {
                "profile_name": entry.role.to_string(),
                "runtime_mode": entry.runtime_mode,
                "labels": entry.labels,
                "execution": { "execution": "controlling_session" }
            },
            "owned_wiring": [],
            "convergence": { "kind": "drain", "max_wait_ms": 5000 }
        }))
        .expect("adoption params");
    let request: meerkat_mob::AdoptMemberIdentityDeclaration =
        params.try_into().expect("adoption request");
    let adoption = handle
        .adopt_member_identity_declaration(request)
        .await
        .expect("the host adopts alice");
    assert!(
        matches!(
            adoption.adoption,
            meerkat_mob::IdentityAdoptionOutcome::Adopted {
                desired_revision: 1
            }
        ),
        "{adoption:?}"
    );
    // Core converges the adopted row on its own. Let it settle, so the
    // snapshot below sees only what the bridge does.
    let converged = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(meerkat_mob::identity::IdentityStoredObservation::Valid(status)) =
                handle.identity_convergence_status(&member).await
                && status.condition() == meerkat_mob::IdentityConvergenceCondition::Converged
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(converged.is_ok(), "Core converges the adopted identity");

    let before = member_snapshot(&handle, &member).await;
    let cursor = last_event_cursor(&handle).await;

    let draft = AgentBuildDraft {
        compaction_curator: Default::default(),
        model: None,
        system_prompt: None,
        additional_instructions: alice.additional_instructions.clone(),
        labels: alice.labels.clone(),
        app_context: alice.context.clone(),
        external_tools: Vec::new(),
        local_external_tools: Default::default(),
        provider_params: None,
    };
    let ensured = bridge.ensure_member_live(&alice.identity).await;
    let resumed = bridge
        .resume_session(&alice.identity, &alice, &draft, &session_id, None)
        .await;
    let host_adopted = |error: Option<&BridgeError>| {
        error.is_some_and(|e| format!("{e:?}").starts_with("HostAdopted"))
    };
    assert!(
        host_adopted(ensured.as_ref().err()) && host_adopted(resumed.as_ref().err()),
        "a host-adopted identity is refused HostAdopted by both verbs: ensure_member_live -> \
         {ensured:?}, resume_session -> {resumed:?}"
    );

    assert_eq!(
        member_snapshot(&handle, &member).await,
        before,
        "nothing moved: the roster entry and the session binding are unchanged"
    );
    let lifecycle: Vec<_> = events_after(&handle, cursor)
        .await
        .into_iter()
        .filter(|event| {
            matches!(
                event.kind,
                meerkat_mob::MobEventKind::MemberSpawned(_)
                    | meerkat_mob::MobEventKind::MemberRetirementStarted { .. }
                    | meerkat_mob::MobEventKind::MemberRetired { .. }
                    | meerkat_mob::MobEventKind::MemberReset { .. }
                    | meerkat_mob::MobEventKind::MemberSessionBindingRecovered(_)
            )
        })
        .collect();
    assert!(
        lifecycle.is_empty(),
        "no member was spawned, retired or reset: {lifecycle:?}"
    );
    unified.shutdown().await;
}

/// Everything about `member` a retire, respawn or ensure would change.
async fn member_snapshot(
    handle: &meerkat_mob::MobHandle,
    member: &meerkat_mob::AgentIdentity,
) -> String {
    let entry = handle
        .get_member(member)
        .await
        .expect("roster read")
        .expect("the member stays on the roster");
    let status = handle
        .list_members_including_retiring()
        .await
        .into_iter()
        .find(|listed| listed.agent_identity == *member)
        .map(|listed| listed.status);
    let session = handle.resolve_bridge_session_id(member).await;
    format!(
        "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
        entry.agent_identity,
        entry.generation,
        entry.fence_token,
        entry.agent_runtime_id,
        entry.role,
        entry.runtime_mode,
        entry.labels,
        status,
        session
    )
}

/// The cursor of the newest event in the mob's event store.
async fn last_event_cursor(handle: &meerkat_mob::MobHandle) -> u64 {
    let mut cursor = 0;
    loop {
        let batch = handle.poll_events(cursor, 512).await.expect("poll events");
        match batch.last() {
            Some(event) => cursor = event.cursor,
            None => return cursor,
        }
    }
}

/// Every event appended after `cursor`.
async fn events_after(
    handle: &meerkat_mob::MobHandle,
    mut cursor: u64,
) -> Vec<meerkat_mob::MobEvent> {
    let mut events = Vec::new();
    loop {
        let batch = handle.poll_events(cursor, 512).await.expect("poll events");
        match batch.last() {
            Some(event) => cursor = event.cursor,
            None => return events,
        }
        events.extend(batch);
    }
}

/// meerkat 0.8.45 reads member status through `observe_member_status_view`,
/// which Meta's production session-service wrapper must forward to the
/// persistent service (served from its live watch). A wrapper answering it
/// with `read` would wait on the member's running turn, and every status read
/// of a busy member would end at the observation deadline with
/// `preview_unavailable = observation_deadline`. So: with a turn held in the
/// model, a status read through the real mob returns the member's last
/// COMMITTED preview, with no preview-unavailable marker.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_members_status_returns_its_committed_preview() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let client = ScriptedClient::immediate(true);
    let (unified, bridge, runtime_id) = boot_one_member(
        &temp.path().join("state"),
        client.clone(),
        Duration::from_secs(30),
    )
    .await;
    let deliver = |text: &'static str| {
        let bridge = Arc::clone(&bridge);
        let runtime_id = runtime_id.clone();
        async move {
            bridge
                .deliver_awaiting_commit_with_mode_context_and_system_prompt(
                    &runtime_id,
                    &meerkat_core::ContentInput::Text(text.to_string()),
                    None,
                    &[],
                    HandlingMode::Queue,
                    None,
                )
                .await
        }
    };

    // One committed turn: its reply ("ok") is the committed preview.
    deliver("first, committed").await.expect("first turn");
    let members = unified.mob_handle().list_all_members().await;
    let member = members
        .into_iter()
        .next()
        .expect("the booted member")
        .agent_identity;

    // A second turn, held in the model: the member is busy.
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *client.entered.lock().await = Some(entered_tx);
    *client.release.lock().await = Some(release_rx);
    let busy = tokio::spawn(deliver("second, held"));
    entered_rx.await.expect("the second turn reaches the model");

    let snapshot = unified
        .mob_handle()
        .member_status(&member)
        .await
        .expect("member status of a busy member");
    assert_eq!(
        snapshot.output_preview.as_deref(),
        Some("ok"),
        "the busy member's committed preview"
    );
    assert_eq!(
        snapshot.preview_unavailable, None,
        "the status read must not have waited on the running turn"
    );

    release_tx.send(()).expect("release the held turn");
    busy.await
        .expect("delivery task")
        .expect("the held turn completes");
    unified.shutdown().await;
}
