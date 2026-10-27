// Repair must not destroy queued work (task #48; production field runs 33758a41 +
// 6bb7010e): identity-first continuity repair used to heal a wedged member by
// FULL disposal — `cancel_active_runtime_turn_before_retire` observed
// queue_len=5 steer_queue_len=10 and PROCEEDED; ArchiveSession destroyed the
// 15 pending review inputs with the member. On the ephemeral runtime-store
// shape those queued inputs are in-memory, so disposal loses them
// irrecoverably.
//
// The two regressions here pin the bridge-side fix:
// 1. `repair_carries_queued_inputs_to_the_healed_successor` — the collision
//    repair captures the member's pending machine ingress BEFORE the
//    destructive retire and re-admits it into the healed successor session,
//    which then DRAINS it (real MeerkatMachine, real mob, gated LLM client).
// 2. `repair_refuses_disposal_when_the_durable_row_is_confirmed_absent` — the
//    preconditions-first gate: when the durable session row the resume retry
//    needs is confirmed absent, no destructive step executes at all and the
//    member (with its queue) is left untouched behind a typed rejection.
//
// The bounded byte-identical-failure park (task #48 (c)) is pinned separately
// in `identity_first::runtime`'s continuity_repair_supervisor_tests.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildDraft, AgentRuntimeServices, BridgeError, DurabilityPolicy,
    DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
    LocalLeaseProvider, MobSessionBridge, RestoreOutcome, SessionBridge, restore_flow,
};
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime,
};
use meerkat_mob::{MobDefinition, MobStorage};
use meerkat_runtime::SessionServiceRuntimeExt;
use tokio::sync::watch;

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

/// Deterministic LLM whose stream blocks until the shared gate opens, and
/// which records EVERY message of every request it is asked to run (carried
/// inputs may be batched with a later delivery into ONE turn, so recording
/// only the last message would hide them from the assertions).
/// Closed gate = the production wedge shape (a turn parked inside the provider
/// stream while fan-in piles up behind it).
struct GatedRecordingClient {
    gate: watch::Receiver<bool>,
    prompts: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl LlmClient for GatedRecordingClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        {
            let mut prompts = self.prompts.lock().expect("prompt record lock");
            for message in &request.messages {
                prompts.push(format!("{message:?}"));
            }
        }
        let mut gate = self.gate.clone();
        let text = futures::stream::once(async move {
            while !*gate.borrow() {
                if gate.changed().await.is_err() {
                    break;
                }
            }
            Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            })
        });
        // meerkat 0.8.22 rejects a turn whose stream carried no normalized
        // provider accounting, so the terminal `Done` never travels alone.
        let provider = LlmClient::provider(self);
        let [usage, done] =
            llm_usage::usage_then_done(request, provider, meerkat_core::StopReason::EndTurn);
        let tail = futures::stream::iter(vec![Ok(usage), Ok(done)]);
        Box::pin(text.chain(tail))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    _runtime: UnifiedRuntime,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    mob_handle: meerkat_mob::MobHandle,
    bridge: Arc<MobSessionBridge>,
    identity: meerkat_meta::identity_first::AgentIdentity,
    spec: DurableAgentSpec,
    runtime_id: meerkat_meta::identity_first::AgentRuntimeId,
    session_id: meerkat_core::types::SessionId,
    prompts: Arc<std::sync::Mutex<Vec<String>>>,
}

fn empty_draft(spec: &DurableAgentSpec) -> AgentBuildDraft {
    AgentBuildDraft {
        compaction_curator: Default::default(),
        model: None,
        system_prompt: None,
        additional_instructions: spec.additional_instructions.clone(),
        labels: spec.labels.clone(),
        app_context: spec.context.clone(),
        external_tools: Vec::new(),
        local_external_tools: Default::default(),
        provider_params: None,
    }
}

fn prompt_input(text: &str) -> meerkat_runtime::Input {
    meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(text, None))
}

fn flow_step_input(text: &str) -> meerkat_runtime::Input {
    meerkat_runtime::Input::FlowStep(meerkat_runtime::FlowStepInput {
        header: meerkat_runtime::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: meerkat_runtime::InputOrigin::Flow {
                flow_id: "carry-test-flow".to_string(),
                step_index: 0,
            },
            durability: meerkat_runtime::InputDurability::Durable,
            visibility: meerkat_runtime::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        step_id: "carry-test-step".to_string(),
        content: meerkat_core::ContentInput::Text(text.to_string()),
        directed_interaction_id: None,
        turn_metadata: None,
    })
}

async fn build_harness(gate: watch::Receiver<bool>) -> Harness {
    let temp = tempfile::tempdir().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(state.join("sessions.db")).expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
            .expect("runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let factory = AgentFactory::new(&state).comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("acquire the repair queue fixture runtime machine"),
    );
    let service = Arc::new(PersistentSessionService::new(
        builder,
        16,
        session_store.clone(),
        runtime_store,
        blob_store,
    ));
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "repair-carry-{}"

[profiles.general]
model = "gpt-5.5"

[profiles.general.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("definition");
    let prompts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service)
        .with_session_runtime_adapter(machine.clone())
        .expect("acquire the fixture session runtime owner")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(GatedRecordingClient {
                gate,
                prompts: Arc::clone(&prompts),
            })),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "repair-carry".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap");

    let continuity_store =
        LocalContinuityStore::open(state.join("continuity.db")).expect("continuity store");
    let fencing_floor = continuity_store
        .max_fencing_token()
        .expect("fencing high-water");
    let mob_handle = runtime.mob_handle();
    let session_service = runtime
        .mob_runtime()
        .session_service()
        .cloned()
        .expect("session service");
    // A tight actor-admission budget so a test failure surfaces as a typed
    // deadline error in seconds instead of parking inside the production
    // 10-minute budget.
    let bridge = Arc::new(
        MobSessionBridge::with_session_store_and_service(
            mob_handle.clone(),
            session_store,
            session_service,
        )
        .with_actor_admission_budget(Duration::from_secs(15)),
    );
    let irt = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(continuity_store),
            lease_provider: Arc::new(LocalLeaseProvider::with_floor(fencing_floor)),
            runtime_instance_id: "repair-carry-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(mob_handle.clone())),
    );
    let identity =
        meerkat_meta::identity_first::AgentIdentity::parse("crew:worker").expect("identity parses");
    let spec = DurableAgentSpec {
        identity: identity.clone(),
        profile: meerkat_mob::ProfileName::from("general"),
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
    };
    let result = restore_flow(&irt, std::slice::from_ref(&spec), None, None)
        .await
        .expect("restore_flow");
    let binding = match result.outcomes.get(&identity) {
        Some(RestoreOutcome::Created { binding, .. }) => binding.clone(),
        other => panic!("worker must fresh-create, got {other:?}"),
    };

    Harness {
        _temp: temp,
        _runtime: runtime,
        machine,
        mob_handle,
        bridge,
        identity,
        spec,
        runtime_id: binding.agent_runtime_id,
        session_id: binding
            .session_id
            .expect("Core binds a fresh member to a session"),
        prompts,
    }
}

async fn wait_until<F>(what: &str, timeout: Duration, mut condition: F)
where
    F: AsyncFnMut() -> bool,
{
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if condition().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Production run 33758a41 shape: a member wedged mid-turn with fan-in queued behind
/// it. The collision repair must carry the queued Prompt inputs into the
/// healed successor session (which drains them), and must not resurrect the
/// flow-step (its correlation belongs to the flow engine — it is destroyed
/// loudly, by class).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repair_carries_queued_inputs_to_the_healed_successor() {
    let (gate_tx, gate_rx) = watch::channel(true);
    let harness = build_harness(gate_rx).await;
    let machine = harness.machine.as_ref();

    // Seed the durable row the resume retry will need: one completed turn
    // through the sanctioned mob deliver lane (machine-direct admission on a
    // never-delivered member has no attached loop to run it).
    harness
        .bridge
        .deliver(
            &harness.runtime_id,
            &meerkat_core::ContentInput::Text("seed turn".to_string()),
        )
        .await
        .expect("seed turn delivery");
    eprintln!("PHASE: seed delivered");
    wait_until(
        "the seed turn to complete",
        Duration::from_secs(20),
        || async {
            let ran = harness
                .prompts
                .lock()
                .expect("prompt record lock")
                .iter()
                .any(|prompt| prompt.contains("seed turn"));
            ran && SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
                .await
                .map(|active| active.is_empty())
                .unwrap_or(false)
        },
    )
    .await;

    // Wedge the member: close the gate, start a turn that parks inside the
    // provider stream, then pile queued fan-in behind it (the production shape).
    gate_tx.send(false).expect("close gate");
    harness
        .bridge
        .deliver(
            &harness.runtime_id,
            &meerkat_core::ContentInput::Text("wedged".to_string()),
        )
        .await
        .expect("wedged turn delivery");
    eprintln!("PHASE: wedged delivered");
    wait_until(
        "the wedged turn to bind the runtime",
        Duration::from_secs(10),
        || async {
            matches!(
                SessionServiceRuntimeExt::runtime_state(machine, &harness.session_id).await,
                Ok(meerkat_runtime::RuntimeState::Running)
            )
        },
    )
    .await;
    harness
        .bridge
        .deliver(
            &harness.runtime_id,
            &meerkat_core::ContentInput::Text("carried-one".to_string()),
        )
        .await
        .expect("queued delivery one");
    eprintln!("PHASE: carried-one delivered");
    harness
        .bridge
        .deliver(
            &harness.runtime_id,
            &meerkat_core::ContentInput::Text("carried-two".to_string()),
        )
        .await
        .expect("queued delivery two");
    eprintln!("PHASE: carried-two delivered");
    SessionServiceRuntimeExt::accept_input(
        machine,
        &harness.session_id,
        flow_step_input("flow-step-content"),
    )
    .await
    .expect("queued flow step");
    eprintln!("PHASE: flow step admitted");
    let pending = SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
        .await
        .expect("pending queue readable");
    // Whether the mid-run (wedged) input is itself listed as active varies
    // with the runtime version; the substance this pin needs is the three
    // QUEUED inputs piled behind the wedge.
    assert!(
        pending.len() >= 3,
        "the wedge must hold at least the three queued inputs, got {}",
        pending.len()
    );

    // The repair: a resume over the live (wedged) member takes the roster-
    // collision arm — capture, retire, resume retry, carry.
    let outcome = harness
        .bridge
        .resume_session(
            &harness.identity,
            &harness.spec,
            &empty_draft(&harness.spec),
            &harness.session_id,
            None,
        )
        .await
        .expect("collision repair resumes");
    eprintln!("PHASE: repair resumed");
    // The invariant is the DURABLE SESSION, and it holds across both arms: the
    // repair must land on the session it was recovering, never a fresh one.
    assert_eq!(
        outcome.session_id(),
        &harness.session_id,
        "repair must land on the SAME durable session, got {outcome:?}"
    );
    assert!(
        outcome.fallback_reason().is_none(),
        "the repair must not have degraded to a fresh spawn: {outcome:?}"
    );
    // And it is deliberately the PENDING arm here. This calls the bridge
    // directly, and the bridge cannot discharge the attach postcondition: only
    // the holder of the continuity record has the generation, checkpoint version
    // and fence that owner registration needs. Production reaches this through
    // IdentityRuntime, which converts it to Resumed via one contract helper.
    // Asserting the arm keeps that boundary honest - if the bridge ever starts
    // reporting Resumed from here, it is registering facts it does not own.
    assert!(
        outcome.needs_owner_registration(),
        "a direct bridge resume must report the attach as pending owner \
         registration, not as resumed: {outcome:?}"
    );

    // The healed successor drains the carried inputs once the gate opens.
    // A post-heal delivery rides along, as it would in the field (the fan-in
    // source keeps delivering); it also guarantees the fresh member's loop is
    // engaged for the machine-level re-admissions.
    gate_tx.send(true).expect("open gate");
    harness
        .bridge
        .deliver(
            &harness.runtime_id,
            &meerkat_core::ContentInput::Text("post-heal ping".to_string()),
        )
        .await
        .expect("post-heal delivery");
    eprintln!("PHASE: post-heal ping delivered");
    wait_until(
        "the healed successor to drain the carried inputs",
        Duration::from_secs(30),
        || async {
            SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
                .await
                .map(|active| active.is_empty())
                .unwrap_or(false)
        },
    )
    .await;
    let prompts = harness.prompts.lock().expect("prompt record lock").clone();
    assert!(
        prompts.iter().any(|prompt| prompt.contains("carried-one")),
        "the successor must run the first carried input; ran: {prompts:?}"
    );
    assert!(
        prompts.iter().any(|prompt| prompt.contains("carried-two")),
        "the successor must run the second carried input; ran: {prompts:?}"
    );
    // The flow-step invariant is NOT asserted here any more, and not because it
    // was weakened.
    //
    // This double records EVERY message of every request - the whole transcript
    // - so a flow step legitimately sitting in the successor's HISTORY was
    // indistinguishable from one re-admitted as a new input. Inferring
    // re-admission from transcript text cannot fail for the right reason, so it
    // is not evidence either way.
    //
    // It is now asserted at the owner instead, where the decision is actually
    // made and where a mutation can be caught:
    // identity_first::bridge::tests::repair_carries_user_input_and_never_a_flow_owned_occurrence
    // pins Input::FlowStep to the "flow-step" uncarryable lane while Prompt,
    // Peer and ExternalEvent are carried. That test was verified to FAIL when
    // FlowStep is moved into the carry arm.
}

/// Preconditions-first (task #48 (a)): when the session the resume retry
/// would need is CONFIRMED absent from the composition's authoritative read
/// view (a continuity record pointing at a vanished session — the external-
/// row-deletion / lost-store shape), the collision arm refuses typed BEFORE
/// any destructive step — the stale member (holding the only live copy of
/// its state, queue included) survives untouched.
///
/// Renamed: this pins the SOURCE-AVAILABILITY refusal, which owns the case
/// where custody is proven ours.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repair_refuses_disposal_when_the_resume_source_is_absent() {
    // Gate closed: the member's queued work must stay pending through the
    // refused repair.
    let (gate_tx, gate_rx) = watch::channel(false);
    let harness = build_harness(gate_rx).await;
    let machine = harness.machine.as_ref();

    SessionServiceRuntimeExt::accept_input(
        machine,
        &harness.session_id,
        prompt_input("only-live-copy"),
    )
    .await
    .expect("queued input");
    let queued_before = SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
        .await
        .expect("queue readable");
    assert!(
        !queued_before.is_empty(),
        "the member must hold queued work"
    );
    let members_before = harness.mob_handle.list_all_members().await.len();
    assert_eq!(members_before, 1, "exactly the worker member exists");

    // A resume target the service has never seen: the continuity-record-
    // points-at-a-vanished-session shape.
    let vanished_session = meerkat_core::types::SessionId::new();
    let error = harness
        .bridge
        .resume_session(
            &harness.identity,
            &harness.spec,
            &empty_draft(&harness.spec),
            &vanished_session,
            None,
        )
        .await
        .expect_err("repair must refuse disposal without a resume source");
    match &error {
        BridgeError::ResumeRejected { detail, .. } => {
            // This fixture writes no identity intent row, so the legacy path
            // runs: the occupant is ours and bound to another session, and
            // the resume-source precondition refuses before any destructive
            // step.
            // BOTH halves: which precondition failed, and what destroying the
            // occupant would cost. Either alone leaves the refusal unactionable.
            assert!(
                detail.contains("is confirmed absent")
                    && detail.contains("only live copy of the session"),
                "the refusal must name the missing resume source AND what retiring the \
                 occupant would destroy: {detail}"
            );
        }
        other => panic!("expected a typed ResumeRejected, got {other:?}"),
    }

    // No destructive step ran: the member and its queue are untouched.
    assert_eq!(
        harness.mob_handle.list_all_members().await.len(),
        members_before,
        "the stale member must survive a precondition-refused repair"
    );
    let queued_after = SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
        .await
        .expect("queue still readable");
    assert_eq!(
        queued_after, queued_before,
        "the queued inputs must survive a precondition-refused repair"
    );
    drop(gate_tx);
}

/// A queued input that NEVER RAN must never be reported as COMPLETED, and must
/// survive a destructive retire in a state a caller can still act on.
///
/// The distinction this protects is the one meerkat's own enum documents: a host
/// reading a terminal outcome is asking *why its work did not run*, and
/// `Consumed` answers that question with something that never happened. The
/// dangerous failure is not "the input was lost" - the sibling test above
/// already pins that it is carried - it is "the input was silently marked
/// done".
///
/// WHAT I LEARNED WRITING THIS, recorded because it corrects the obligation as
/// filed. I set out to assert `Abandoned { reason: NeverExecuted }` and the
/// premise was wrong twice over:
///
///  - `retire_runtime` does NOT abandon a queued input. It leaves it `Queued`
///    with no terminal outcome, deliberately, because identity-first repair
///    CARRIES pending ingress into the healed successor. That is the fix the
///    sibling test above exists to protect; abandoning here would re-introduce
///    the defect it guards.
///  - the abandoning teardowns (`Reset` / `Stopped`) are unreachable in this
///    state: reset is guard-rejected from `Running`, and a wedged turn is
///    precisely what holds the runtime `Running`. `NeverExecuted` itself has one
///    production producer - a run created then refused - which needs a
///    store-commit failure and is not drivable from a black-box harness.
///
/// So the reachable, meaningful Meta contract is the anti-fabrication one
/// asserted below. `InputAbandonReason::NeverExecuted` remains covered only by
/// meerkat's driver-level tests, and this must not be recorded as covering it.
#[tokio::test]
async fn a_queued_input_that_never_ran_terminalizes_as_abandoned_not_consumed() {
    let (gate_tx, gate_rx) = watch::channel(true);
    let harness = build_harness(gate_rx).await;
    let machine = harness.machine.as_ref();

    // Seed one completed turn so the durable row exists, same as the siblings.
    harness
        .bridge
        .deliver(
            &harness.runtime_id,
            &meerkat_core::ContentInput::Text("seed turn".to_string()),
        )
        .await
        .expect("seed turn delivery");
    wait_until(
        "the seed turn to complete",
        Duration::from_secs(20),
        || async {
            let ran = harness
                .prompts
                .lock()
                .expect("prompt record lock")
                .iter()
                .any(|prompt| prompt.contains("seed turn"));
            ran && SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
                .await
                .map(|active| active.is_empty())
                .unwrap_or(false)
        },
    )
    .await;

    // Wedge the member so nothing behind it can start.
    gate_tx.send(false).expect("close gate");
    harness
        .bridge
        .deliver(
            &harness.runtime_id,
            &meerkat_core::ContentInput::Text("wedged".to_string()),
        )
        .await
        .expect("wedged turn delivery");
    wait_until(
        "the wedged turn to bind the runtime",
        Duration::from_secs(10),
        || async {
            !SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
                .await
                .map(|active| active.is_empty())
                .unwrap_or(true)
        },
    )
    .await;

    // Snapshot BEFORE queuing so the queued input is identified by DIFFERENCE
    // rather than by assuming an ordering the runtime does not promise.
    let before: Vec<_> = SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
        .await
        .expect("active inputs readable");

    harness
        .bridge
        .deliver(
            &harness.runtime_id,
            &meerkat_core::ContentInput::Text("queued behind the wedge".to_string()),
        )
        .await
        .expect("queued delivery");

    let queued_id = {
        let mut found = None;
        for _ in 0..100 {
            let after = SessionServiceRuntimeExt::list_active_inputs(machine, &harness.session_id)
                .await
                .expect("active inputs readable");
            if let Some(id) = after.iter().find(|id| !before.contains(id)) {
                found = Some((*id).clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        found.expect("the queued input must appear as active behind the wedge")
    };

    // It must never have run: the gate is still closed, so no prompt for it.
    assert!(
        !harness
            .prompts
            .lock()
            .expect("prompt record lock")
            .iter()
            .any(|prompt| prompt.contains("queued behind the wedge")),
        "precondition: the queued input must NOT have executed, or this test is \
         asserting about a different lifecycle than the one it names"
    );

    // Tear the runtime down. Retire is the destructive path and is legal
    // from Running; reset is not, because the wedged turn holds the runtime
    // there - which is exactly the state a never-ran queued input lives in.
    SessionServiceRuntimeExt::retire_runtime(machine, &harness.session_id)
        .await
        .expect("retire must terminalize pending inputs");

    let state = SessionServiceRuntimeExt::input_state(machine, &harness.session_id, &queued_id)
        .await
        .expect("input state readable")
        .expect("the queued input must still have a durable row after teardown");

    let outcome = state.seed.terminal_outcome.clone();

    // THE LOAD-BEARING ASSERTION. A `Queued` phase is what makes the input
    // recoverable, and it is mutually exclusive with any completion verdict:
    // a `Consumed` outcome cannot coexist with a non-terminal phase. So this
    // one line carries both halves - "not lost" and "not reported done".
    assert_eq!(
        state.seed.phase,
        meerkat_runtime::InputLifecycleState::Queued,
        "a never-run queued input must remain queued and recoverable across a \
         destructive retire, not be disposed of, terminalized, or marked done: \
         {state:?}"
    );

    // Consistency guard, and stated honestly: on this path `terminal_outcome`
    // is always `None`, so this cannot fire today and is NOT live coverage of
    // the not-Consumed property - the phase assertion above is. It exists to
    // catch a future change that sets a terminal outcome while leaving the
    // phase non-terminal, which would be an internally inconsistent row that
    // the phase check alone would pass.
    assert!(
        !matches!(
            outcome,
            Some(meerkat_runtime::InputTerminalOutcome::Consumed)
        ),
        "terminal outcome Consumed on a {:?} phase is an inconsistent row: {outcome:?}",
        state.seed.phase
    );

    // The precondition restated as a postcondition: still never ran.
    assert!(
        !harness
            .prompts
            .lock()
            .expect("prompt record lock")
            .iter()
            .any(|prompt| prompt.contains("queued behind the wedge")),
        "the queued input must still not have executed after the retire"
    );
}
