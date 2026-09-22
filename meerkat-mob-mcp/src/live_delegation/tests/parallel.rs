//! Offline tests for WorkGraph-backed parallel live delegation.
//!
//! A scripted LLM client stands in for every forked worker and for the source
//! member: each call blocks until the test releases it, so the tests control
//! exactly when a worker finishes. WorkGraph dependency behaviour is driven
//! through the same `WorkGraphService` the forks' tools would use.

use super::*;
use futures::StreamExt as _;
use meerkat_mob::MobSessionService as _;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Workspace(std::path::PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One model call observed by the scripted client: its index and the text
/// of the user-role messages it carried.
#[derive(Debug, Clone)]
struct ObservedCall {
    index: usize,
    user_text: String,
}

struct ScriptedClient {
    calls: AtomicUsize,
    entered: tokio::sync::mpsc::UnboundedSender<ObservedCall>,
    cancelled: tokio::sync::mpsc::UnboundedSender<usize>,
    gates: std::sync::Mutex<HashMap<usize, Arc<tokio::sync::Semaphore>>>,
}

impl ScriptedClient {
    fn new() -> (
        Arc<Self>,
        tokio::sync::mpsc::UnboundedReceiver<ObservedCall>,
        tokio::sync::mpsc::UnboundedReceiver<usize>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancelled_tx, cancelled_rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                entered: entered_tx,
                cancelled: cancelled_tx,
                gates: std::sync::Mutex::new(HashMap::new()),
            }),
            entered_rx,
            cancelled_rx,
        )
    }

    fn gate(&self, index: usize) -> Arc<tokio::sync::Semaphore> {
        Arc::clone(
            self.gates
                .lock()
                .expect("gate map")
                .entry(index)
                .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(0))),
        )
    }

    /// Let call `index` finish with the text `completed:<index>`.
    fn release(&self, index: usize) {
        self.gate(index).add_permits(1);
    }
}

struct InFlightCall {
    index: usize,
    cancelled: tokio::sync::mpsc::UnboundedSender<usize>,
    completed: bool,
}

impl Drop for InFlightCall {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.cancelled.send(self.index);
        }
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for ScriptedClient {
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
        let user_text = request
            .messages
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let response = futures::stream::once(async move {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            let mut call = InFlightCall {
                index,
                cancelled: self.cancelled.clone(),
                completed: false,
            };
            let _ = self.entered.send(ObservedCall { index, user_text });
            self.gate(index)
                .acquire()
                .await
                .expect("scripted gate")
                .forget();
            call.completed = true;
            Ok::<_, meerkat_client::LlmError>(meerkat_client::LlmEvent::TextDelta {
                delta: format!("completed:{index}"),
                meta: None,
            })
        });
        Box::pin(response.chain(futures::stream::once(async {
            Ok(meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            })
        })))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

struct Fixture {
    _workspace: Workspace,
    client: Arc<ScriptedClient>,
    entered: tokio::sync::mpsc::UnboundedReceiver<ObservedCall>,
    cancelled: tokio::sync::mpsc::UnboundedReceiver<usize>,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    handle: MobHandle,
    identity: AgentIdentity,
    session_id: SessionId,
    coordinator: Arc<ExperimentalLiveDelegationCoordinator>,
    control: Arc<ExactProjectionControl>,
    binding: LiveDelegationRuntimeBinding,
    provider_binding: ProviderWebrtcBinding,
    workgraph: Option<meerkat::WorkGraphService>,
}

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const QUIET: std::time::Duration = std::time::Duration::from_millis(1500);

async fn fixture(with_workgraph: bool) -> Fixture {
    fixture_with_policy(with_workgraph, LiveDelegationExecutionPolicy::DurableFork).await
}

async fn fixture_with_policy(
    with_workgraph: bool,
    policy: LiveDelegationExecutionPolicy,
) -> Fixture {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "meerkat_mob_mcp::live_delegation=debug,meerkat_mob::runtime::delegation=debug"
                    .into()
            }),
        )
        .with_test_writer()
        .try_init();
    let workspace = Workspace(std::path::PathBuf::from(format!(
        ".live-parallel-{}",
        uuid::Uuid::new_v4(),
    )));
    std::fs::create_dir(&workspace.0).expect("project-local test root");
    for name in ["user", "runtime", "project", "context"] {
        std::fs::create_dir(workspace.0.join(name)).expect("factory root");
    }
    let (client, entered, cancelled) = ScriptedClient::new();
    // The mob's shared WorkGraph is scoped to the mob realm, exactly as a
    // host wires it: members build in `mob.<id>` and the grant must match.
    let mob_name = format!("parallel-{}", uuid::Uuid::new_v4());
    let mob_realm = meerkat_core::mob_realm_id(&format!("implicit-{mob_name}"))
        .expect("mob realm")
        .as_str()
        .to_string();
    let factory = meerkat::AgentFactory::new(workspace.0.join("factory"))
        .user_config_root(workspace.0.join("user"))
        .runtime_root(workspace.0.join("runtime"))
        .project_root(workspace.0.join("project"))
        .context_root(workspace.0.join("context"))
        .builtins(false)
        .comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    builder.default_llm_client = Some(client.clone());
    let workgraph = with_workgraph.then(|| {
        meerkat::WorkGraphService::with_scope(
            Arc::new(meerkat::MemoryWorkGraphStore::new()),
            mob_realm.clone(),
            meerkat::WorkNamespace::default(),
        )
    });
    if let Some(service) = workgraph.as_ref() {
        meerkat::surface::set_default_workgraph_namespace_grant(
            &builder,
            Some(service.namespace_grant().clone()),
        );
        meerkat::surface::set_default_workgraph_tools(
            &builder,
            Some(Arc::new(meerkat::WorkGraphToolSurface::new(
                service.clone(),
            ))),
        );
    }
    let store = Arc::new(meerkat_store::MemoryStore::new());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let runtime = service.runtime_adapter().expect("runtime");
    let mobs = Arc::new(
        crate::MobMcpState::new(service.clone(), meerkat_mob::MobControlPrincipal::Owner)
            .with_workgraph_service(workgraph.clone()),
    );
    let mob_id = mobs
        .mob_create_definition(meerkat_mob::MobDefinition::implicit(&mob_name, "gpt-5.5"))
        .await
        .expect("mob");
    let identity = AgentIdentity::from("voice-source");
    let mut spec = meerkat_mob::SpawnMemberSpec::new("delegate", identity.as_str());
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    mobs.mob_spawn_spec(&mob_id, spec)
        .await
        .expect("source member");
    let handle = mobs.handle_for(&mob_id).await.expect("mob handle");
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("source session");
    let coordinator = Arc::new(
        ExperimentalLiveDelegationCoordinator::new(Arc::clone(&runtime), mobs)
            .with_execution_policy(policy),
    );
    let control = Arc::new(ExactProjectionControl::default());
    let binding = runtime
        .__test_open_live_delegation_channel(&session_id)
        .await
        .expect("binding");
    let provider_binding = provider_binding_from_runtime(&binding);
    Fixture {
        _workspace: workspace,
        client,
        entered,
        cancelled,
        runtime,
        handle,
        identity,
        session_id,
        coordinator,
        control,
        binding,
        provider_binding,
        workgraph,
    }
}

impl Fixture {
    /// Drive one provider user turn that ends in a client delegation, then
    /// hand the joined delegation to the coordinator.
    async fn delegate(&self, key: &str, transcript: &str) -> OperationId {
        let turn = LiveSidebandTurnRef::__from_provider_observation(
            self.binding.channel_id(),
            format!("{key}-turn"),
            format!("{key}-provider-turn"),
        )
        .expect("turn");
        let delegation = LiveSidebandDelegationRef::__from_provider_observation(
            format!("{key}-delegation"),
            format!("{key}-provider-delegation"),
        )
        .expect("delegation");
        self.coordinator
            .observe_turn_started(&LiveSidebandObservation::new(
                self.provider_binding.clone(),
                LiveSidebandObservationKind::TurnStarted {
                    turn: turn.clone(),
                    role: meerkat_live::LiveSidebandTurnRole::User,
                },
            ))
            .await
            .expect("user turn starts");
        self.coordinator
            .observe_delegation_turn_finished(
                &LiveSidebandObservation::new(
                    self.provider_binding.clone(),
                    LiveSidebandObservationKind::TurnFinished {
                        turn: turn.clone(),
                        role: meerkat_live::LiveSidebandTurnRole::User,
                        transcript: transcript.to_string(),
                    },
                ),
                &delegation,
                transcript,
            )
            .await
            .expect("delegation turn finished");
        let operation = self
            .coordinator
            .completed_delegation_turns
            .lock()
            .await
            .get(&(
                self.session_id.clone(),
                self.binding.channel_id().clone(),
                turn.adapter_key().to_string(),
            ))
            .expect("operation admitted")
            .operation
            .operation_id()
            .clone();
        let control: Arc<dyn ExperimentalGptLiveControlPlane> =
            Arc::clone(&self.control) as Arc<dyn ExperimentalGptLiveControlPlane>;
        self.coordinator
            .start_client_context_delegation(
                &self.provider_binding,
                control,
                turn,
                delegation,
                transcript.to_string(),
            )
            .await
            .expect("delegation scheduled");
        operation
    }

    async fn next_call(&mut self) -> ObservedCall {
        tokio::time::timeout(WAIT, self.entered.recv())
            .await
            .expect("a model call starts in time")
            .expect("client alive")
    }

    async fn expect_no_call(&mut self) {
        assert!(
            tokio::time::timeout(QUIET, self.entered.recv())
                .await
                .is_err(),
            "no model call must start while the schedule is full or serial"
        );
    }

    fn assert_nothing_cancelled(&mut self) {
        assert!(
            self.cancelled.try_recv().is_err(),
            "no worker may be cancelled by arrival or close"
        );
    }

    async fn schedule_state(
        &self,
        operation: &OperationId,
    ) -> Option<meerkat_runtime::live_execution::LiveDelegationScheduleState> {
        self.runtime
            .live_delegation_schedule_state(&self.session_id, operation)
            .await
            .expect("schedule state readable")
    }

    /// Result delivery to the provider needs the experimental context
    /// binding this fixture does not open, so completion is observed through
    /// the generated schedule state instead of provider releases.
    async fn wait_for_completed(&self, operations: &[OperationId]) {
        wait_until(WAIT, || async {
            let mut all = true;
            for operation in operations {
                all &= self.schedule_state(operation).await
                    == Some(
                        meerkat_runtime::live_execution::LiveDelegationScheduleState::Completed,
                    );
            }
            all
        })
        .await;
    }

    async fn narrations(&self) -> Vec<(LiveDelegationNarrationKind, String)> {
        self.control.narrations.lock().await.clone()
    }

    async fn voice_items(&self) -> Vec<meerkat::WorkItem> {
        self.workgraph
            .as_ref()
            .expect("workgraph")
            .list(meerkat::WorkItemFilter {
                labels: vec![schedule::VOICE_WORK_LABEL.to_string()],
                include_terminal: true,
                ..meerkat::WorkItemFilter::default()
            })
            .await
            .expect("voice items")
    }

    async fn voice_item_titled(&self, title: &str) -> meerkat::WorkItem {
        self.voice_items()
            .await
            .into_iter()
            .find(|item| item.title == title)
            .unwrap_or_else(|| unreachable!("voice item {title:?} exists"))
    }

    async fn wait_for_retired(&self, expected: usize) {
        wait_until(WAIT, || async {
            let snapshots = self
                .runtime
                .live_delegation_recovery_snapshots(&self.session_id)
                .await
                .expect("snapshots");
            snapshots.len() == expected
                && snapshots.iter().all(|snapshot| {
                    snapshot.phase()
                        == meerkat_runtime::live_execution::LiveDelegationRecoveryPhase::Retired
                })
        })
        .await;
    }

    async fn close(self) {
        assert_eq!(
            self.handle.resolve_bridge_session_id(&self.identity).await,
            Some(self.session_id.clone()),
            "the source member keeps its session through voice scheduling"
        );
        self.coordinator
            .cancel_channel_binding(&self.provider_binding)
            .await;
        self.runtime
            .abandon_live_open_admission(&self.session_id, self.binding.channel_id())
            .await
            .expect("close fixture channel");
        self.handle.shutdown().await.expect("shutdown");
    }
}

async fn wait_until<F, Fut>(timeout: std::time::Duration, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(timeout, async {
        loop {
            if condition().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition holds before the timeout");
}

fn kinds(narrations: &[(LiveDelegationNarrationKind, String)]) -> Vec<LiveDelegationNarrationKind> {
    narrations.iter().map(|(kind, _)| *kind).collect()
}

#[tokio::test]
async fn two_delegations_run_in_parallel_and_both_complete() {
    let mut fx = fixture(true).await;
    let first = fx.delegate("first", "find the fastest train to Oslo").await;
    let second = fx
        .delegate("second", "summarize the budget spreadsheet")
        .await;

    // Both forks reach the model before either finishes: arrival never
    // cancels or waits for the earlier delegation.
    let a = fx.next_call().await;
    let b = fx.next_call().await;
    assert_eq!(
        [a.index, b.index]
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        [0, 1].into_iter().collect()
    );
    fx.assert_nothing_cancelled();
    assert_eq!(
        fx.schedule_state(&first).await,
        Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Running)
    );
    assert_eq!(
        fx.schedule_state(&second).await,
        Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Running)
    );

    fx.client.release(1);
    fx.client.release(0);
    fx.wait_for_completed(&[first.clone(), second.clone()])
        .await;
    fx.wait_for_retired(2).await;
    fx.assert_nothing_cancelled();

    // Each delegation was narrated as started and as finished.
    wait_until(WAIT, || async {
        fx.narrations()
            .await
            .iter()
            .filter(|(kind, _)| *kind == LiveDelegationNarrationKind::Completed)
            .count()
            == 2
    })
    .await;
    let narrations = fx.narrations().await;
    let claimed = narrations
        .iter()
        .filter(|(kind, _)| *kind == LiveDelegationNarrationKind::Claimed)
        .count();
    let completed = narrations
        .iter()
        .filter(|(kind, _)| *kind == LiveDelegationNarrationKind::Completed)
        .count();
    assert_eq!((claimed, completed), (2, 2), "{narrations:?}");
    assert!(
        narrations.iter().any(
            |(kind, text)| *kind == LiveDelegationNarrationKind::Completed
                && text.contains("find the fastest train to Oslo")
        ),
        "{narrations:?}"
    );

    let items = fx.voice_items().await;
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .all(|item| item.status == meerkat::WorkStatus::Completed)
    );
    assert!(items.iter().all(|item| {
        item.evidence_refs
            .iter()
            .any(|evidence| evidence.kind == "live_delegation_result")
    }));
    fx.close().await;
}

#[tokio::test]
async fn fifth_delegation_waits_for_a_worker_slot_and_is_narrated_queued() {
    let mut fx = fixture(true).await;
    let mut operations = Vec::new();
    for index in 0..5 {
        operations.push(
            fx.delegate(&format!("d{index}"), &format!("voice task number {index}"))
                .await,
        );
    }
    let mut running = std::collections::BTreeSet::new();
    for _ in 0..4 {
        running.insert(fx.next_call().await.index);
    }
    assert_eq!(running, (0..4).collect());
    fx.expect_no_call().await;
    assert_eq!(
        fx.schedule_state(&operations[4]).await,
        Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Created)
    );
    wait_until(WAIT, || async {
        fx.narrations()
            .await
            .iter()
            .any(|(kind, _)| *kind == LiveDelegationNarrationKind::Queued)
    })
    .await;
    let queued = fx
        .narrations()
        .await
        .into_iter()
        .find(|(kind, _)| *kind == LiveDelegationNarrationKind::Queued)
        .expect("queued narration");
    assert_eq!(
        queued.1,
        "Voice request queued: \"voice task number 4\". 4 request(s) are running ahead of it; it starts when a slot frees."
    );

    // Finishing one worker frees a slot; the queued item starts.
    fx.client.release(2);
    let fifth = fx.next_call().await;
    assert_eq!(fifth.index, 4);
    assert!(fifth.user_text.contains("voice task number 4"));
    for index in [0, 1, 3, 4] {
        fx.client.release(index);
    }
    fx.wait_for_completed(&operations).await;
    fx.wait_for_retired(5).await;
    fx.assert_nothing_cancelled();
    fx.close().await;
}

#[tokio::test]
async fn blocked_worker_is_retired_and_requeued_with_the_dependency_result() {
    let mut fx = fixture(true).await;
    let first = fx.delegate("first", "collect the quarterly numbers").await;
    let second = fx
        .delegate("second", "write the summary from the numbers")
        .await;
    let first_call = fx.next_call().await;
    let second_call = fx.next_call().await;
    assert_eq!(
        [first_call.index, second_call.index]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        [0, 1].into_iter().collect()
    );
    let first_call_index = if first_call.user_text.contains("quarterly") {
        first_call.index
    } else {
        second_call.index
    };
    let second_call_index = 1 - first_call_index;
    let fork_instructions_seen = [&first_call, &second_call].iter().all(|call| {
        call.user_text.contains("collect the quarterly")
            || call.user_text.contains("write the summary")
    });
    assert!(fork_instructions_seen);

    // The second worker declares its dependency exactly the way its briefing
    // says: a `blocks` edge from the first item, then release, then end turn.
    let workgraph = fx.workgraph.clone().expect("workgraph");
    let first_item = fx.voice_item_titled("collect the quarterly numbers").await;
    let second_item = fx
        .voice_item_titled("write the summary from the numbers")
        .await;
    assert_eq!(second_item.status, meerkat::WorkStatus::InProgress);
    workgraph
        .link(meerkat::LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            kind: meerkat::WorkEdgeKind::Blocks,
            from_id: first_item.id.clone(),
            to_id: second_item.id.clone(),
        })
        .await
        .expect("blocks edge");
    let second_item = fx
        .voice_item_titled("write the summary from the numbers")
        .await;
    workgraph
        .release(meerkat::ReleaseWorkItemRequest {
            id: second_item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: second_item.revision,
        })
        .await
        .expect("release behind the blocker");
    fx.client.release(second_call_index);

    wait_until(WAIT, || async {
        fx.schedule_state(&second).await
            == Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Blocked)
    })
    .await;
    wait_until(WAIT, || async {
        fx.narrations()
            .await
            .iter()
            .any(|(kind, _)| *kind == LiveDelegationNarrationKind::Blocked)
    })
    .await;
    let blocked = fx
        .narrations()
        .await
        .into_iter()
        .find(|(kind, _)| *kind == LiveDelegationNarrationKind::Blocked)
        .expect("blocked narration");
    assert_eq!(
        blocked.1,
        "Voice request \"write the summary from the numbers\" is waiting for \"collect the quarterly numbers\" to finish first."
    );
    // Nothing was released to the provider for the blocked worker.
    assert!(fx.control.releases.lock().await.is_empty());
    fx.expect_no_call().await;

    // The dependency finishes: its item closes, the blocked item becomes
    // ready, the same operation binds a fresh worker with the result.
    fx.client.release(first_call_index);
    let restarted = fx.next_call().await;
    assert_eq!(restarted.index, 2);
    assert!(
        restarted
            .user_text
            .contains("Results of the voice requests this one waited for"),
        "{}",
        restarted.user_text
    );
    assert!(
        restarted
            .user_text
            .contains(&format!("completed:{first_call_index}")),
        "{}",
        restarted.user_text
    );
    assert!(
        restarted
            .user_text
            .contains("write the summary from the numbers")
    );
    assert_eq!(
        fx.schedule_state(&second).await,
        Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Running)
    );
    fx.client.release(2);
    fx.wait_for_completed(&[first.clone(), second.clone()])
        .await;
    fx.assert_nothing_cancelled();
    wait_until(WAIT, || async {
        fx.narrations()
            .await
            .iter()
            .filter(|(kind, _)| *kind == LiveDelegationNarrationKind::Completed)
            .count()
            == 2
    })
    .await;
    let second_narrations = fx
        .narrations()
        .await
        .into_iter()
        .filter(|(_, text)| text.contains("write the summary from the numbers"))
        .collect::<Vec<_>>();
    assert_eq!(
        kinds(&second_narrations),
        vec![
            LiveDelegationNarrationKind::Claimed,
            LiveDelegationNarrationKind::Blocked,
            LiveDelegationNarrationKind::Claimed,
            LiveDelegationNarrationKind::Completed,
        ],
        "{second_narrations:?}"
    );
    let second_item = fx
        .voice_item_titled("write the summary from the numbers")
        .await;
    assert_eq!(second_item.status, meerkat::WorkStatus::Completed);
    fx.close().await;
}

#[tokio::test]
async fn channel_close_cancels_only_queued_work_and_running_forks_merge_into_the_source() {
    let mut fx = fixture(true).await;
    let mut operations = Vec::new();
    for index in 0..5 {
        operations.push(
            fx.delegate(&format!("c{index}"), &format!("long task {index}"))
                .await,
        );
    }
    for _ in 0..4 {
        fx.next_call().await;
    }
    fx.expect_no_call().await;

    fx.coordinator
        .cancel_channel_binding(&fx.provider_binding)
        .await;
    fx.runtime
        .abandon_live_open_admission(&fx.session_id, fx.binding.channel_id())
        .await
        .expect("channel closes");

    // Only the queued item was cancelled, in the machine and in the WorkGraph.
    assert_eq!(
        fx.schedule_state(&operations[4]).await,
        Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Cancelled)
    );
    assert_eq!(
        fx.voice_item_titled("long task 4").await.status,
        meerkat::WorkStatus::Cancelled
    );
    for operation in &operations[..4] {
        assert_eq!(
            fx.schedule_state(operation).await,
            Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Running)
        );
    }
    fx.assert_nothing_cancelled();

    // Running forks finish after the close; each result merges into the
    // source member as ordinary internal work instead of being dropped.
    for index in 0..4 {
        fx.client.release(index);
    }
    // The source member runs one queued turn at a time, so each merge call
    // is released before the next one can start.
    for _ in 0..4 {
        let call = fx.next_call().await;
        assert!(
            call.user_text
                .contains("finished after the voice call ended"),
            "{}",
            call.user_text
        );
        fx.client.release(call.index);
    }
    fx.wait_for_retired(4).await;
    let snapshots = fx
        .runtime
        .live_delegation_recovery_snapshots(&fx.session_id)
        .await
        .expect("snapshots");
    assert!(snapshots.iter().all(|snapshot| {
        snapshot.terminal() == Some(LiveDelegationWorkerTerminalKind::Completed) && snapshot.late()
    }));
    assert!(
        fx.control.releases.lock().await.is_empty(),
        "nothing reaches the closed provider channel"
    );
    for title in (0..4).map(|index| format!("long task {index}")) {
        assert_eq!(
            fx.voice_item_titled(&title).await.status,
            meerkat::WorkStatus::Completed
        );
    }
    fx.assert_nothing_cancelled();
    fx.handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn without_a_workgraph_store_delegations_run_strictly_serially_and_never_supersede() {
    let mut fx = fixture(false).await;
    let first = fx.delegate("first", "serial task one").await;
    let second = fx.delegate("second", "serial task two").await;
    let head = fx.next_call().await;
    assert_eq!(head.index, 0);
    assert!(head.user_text.contains("serial task one"));
    fx.expect_no_call().await;
    assert_eq!(
        fx.schedule_state(&second).await,
        Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Created)
    );
    fx.assert_nothing_cancelled();

    fx.client.release(0);
    let tail = fx.next_call().await;
    assert_eq!(tail.index, 1);
    assert!(tail.user_text.contains("serial task two"));
    fx.client.release(1);
    fx.wait_for_completed(&[first.clone(), second.clone()])
        .await;
    fx.wait_for_retired(2).await;
    fx.assert_nothing_cancelled();
    let narrations = fx.narrations().await;
    assert_eq!(
        kinds(&narrations)
            .into_iter()
            .filter(|kind| *kind == LiveDelegationNarrationKind::Queued)
            .count(),
        1,
        "{narrations:?}"
    );
    fx.close().await;
}

// The DurableFork path on fix/voice-continuing-conversation f77099a23 hangs
// when the source member is mid-turn (the fork under the held turn boundary
// never returns), so the SourceBusy result this test drives is unreachable
// until that fix lands. Un-ignore after rebasing onto it.
#[tokio::test]
#[ignore = "blocked on the voice branch fork-under-held-guard hang; SourceBusy never surfaces on f77099a23"]
async fn busy_source_member_defers_the_fork_narrates_and_retries_without_dropping_it() {
    let mut fx = fixture(true).await;
    // The source member is mid-turn when the delegation arrives, so the
    // bounded fork wait ends in SourceBusy.
    let busy = fx
        .handle
        .start_work_for_identity_bounded(
            fx.identity.clone(),
            WorkSpec::new("long running text turn", WorkOrigin::Internal),
            meerkat_core::types::HandlingMode::Queue,
            BoundedResultSpec::new("busy", 256).expect("bound"),
        )
        .await
        .expect("queue source text");
    let source_call = fx.next_call().await;
    assert!(source_call.user_text.contains("long running text turn"));

    let operation = fx.delegate("busy", "task while the source is busy").await;
    let busy_wait = meerkat_mob::DelegationExecutionService::SOURCE_TURN_BOUNDARY_WAIT
        + std::time::Duration::from_secs(20);
    wait_until(busy_wait, || async {
        fx.narrations()
            .await
            .iter()
            .any(|(kind, _)| *kind == LiveDelegationNarrationKind::SourceBusy)
    })
    .await;
    assert_eq!(
        fx.schedule_state(&operation).await,
        Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Created)
    );
    fx.assert_nothing_cancelled();

    // The source finishes; the deferred delegation starts on the retry.
    fx.client.release(source_call.index);
    tokio::time::timeout(
        WAIT,
        busy.wait_bounded(BoundedResultSpec::new("busy", 256).expect("bound")),
    )
    .await
    .expect("source turn completes")
    .expect("source result");
    let fork = tokio::time::timeout(busy_wait, fx.entered.recv())
        .await
        .expect("retried fork reaches the model")
        .expect("client alive");
    assert!(
        fork.user_text.contains("task while the source is busy"),
        "{}",
        fork.user_text
    );
    fx.client.release(fork.index);
    fx.wait_for_completed(&[operation]).await;
    fx.assert_nothing_cancelled();
    fx.close().await;
}

/// Bug 1 (S107): closing the channel while a delegation runs must not wait
/// for the worker. The control consumer finishes at once, so the provider
/// drain can settle inside its bound, and an existing member's result stays
/// in its own canonical session (no merge turn is queued).
#[tokio::test]
async fn channel_close_returns_without_awaiting_a_running_existing_member_turn() {
    let mut fx = fixture_with_policy(true, LiveDelegationExecutionPolicy::ExistingMember).await;
    let operation = fx.delegate("em", "existing member task").await;
    let running = fx.next_call().await;
    assert!(running.user_text.contains("existing member task"));

    let close_started = std::time::Instant::now();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fx.coordinator.cancel_channel_binding(&fx.provider_binding),
    )
    .await
    .expect("close never waits for the running turn");
    assert!(close_started.elapsed() < std::time::Duration::from_secs(2));
    fx.runtime
        .abandon_live_open_admission(&fx.session_id, fx.binding.channel_id())
        .await
        .expect("channel closes");
    fx.assert_nothing_cancelled();
    assert_eq!(
        fx.schedule_state(&operation).await,
        Some(meerkat_runtime::live_execution::LiveDelegationScheduleState::Running),
        "the running turn keeps its custody through the close"
    );

    fx.client.release(running.index);
    fx.wait_for_retired(1).await;
    let snapshots = fx
        .runtime
        .live_delegation_recovery_snapshots(&fx.session_id)
        .await
        .expect("snapshots");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        snapshots[0].worker_ownership(),
        LiveDelegationWorkerOwnership::ExistingMember
    );
    assert_eq!(
        snapshots[0].terminal(),
        Some(LiveDelegationWorkerTerminalKind::Completed)
    );
    // The member's own turn already carries the result: no merge turn.
    fx.expect_no_call().await;
    assert_eq!(
        fx.handle.resolve_bridge_session_id(&fx.identity).await,
        Some(fx.session_id.clone()),
        "an existing member is never retired by the post-close path"
    );
    fx.handle.shutdown().await.expect("shutdown");
}
