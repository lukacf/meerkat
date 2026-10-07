//! Additional owner-backed workloads; no production hooks or permission substitutes.
use super::*;
use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
use meerkat_core::{HandlingMode, InputId, RunId};
use meerkat_runtime::input_state::{InputAbandonReason, InputTerminalOutcome};
use std::collections::BTreeSet;

const OLD_ROWS: usize = 252;
const CONTRIBUTORS: usize = 4;
const PREFIX_READS: usize = 333;
const PREFIX_RECORDS: usize = 1002;

#[derive(Debug, PartialEq, Eq)]
struct RepresentativeMeasurementSettings {
    profile: &'static str,
    warmup: usize,
    pairs: usize,
}

fn representative_measurement_settings(
    profile: Option<&str>,
    warmup: Option<usize>,
    pairs: Option<usize>,
) -> Result<RepresentativeMeasurementSettings, &'static str> {
    let (profile, warmup, pairs) = match profile {
        Some("tail") => return Err("representative tail profile is withdrawn; use fixed_mean_32"),
        None | Some("fixed_mean_32") => {
            if warmup.is_some_and(|value| value != 20) || pairs.is_some_and(|value| value != 32) {
                return Err("fixed mean profile requires exactly 20 warmup and 32 measured pairs");
            }
            ("fixed_mean_32", 20, 32)
        }
        _ => return Err("unknown representative measurement profile"),
    };
    Ok(RepresentativeMeasurementSettings {
        profile,
        warmup,
        pairs,
    })
}

async fn with_representative_deadline<T>(
    deadline: tokio::time::Instant,
    work: impl std::future::Future<Output = T>,
) -> Result<T, ()> {
    if tokio::time::Instant::now() >= deadline {
        return Err(());
    }
    let result = tokio::time::timeout_at(deadline, work)
        .await
        .map_err(|_| ())?;
    // A synchronous poll (including output I/O) cannot be preempted by Tokio.
    // Reject its late completion; the external process deadline bounds it.
    if tokio::time::Instant::now() >= deadline {
        return Err(());
    }
    Ok(result)
}

#[test]
fn representative_fixed_mean_profile_keeps_exact_counts_separate_from_tail() {
    let expected = RepresentativeMeasurementSettings {
        profile: "fixed_mean_32",
        warmup: 20,
        pairs: 32,
    };
    assert_eq!(
        representative_measurement_settings(Some("fixed_mean_32"), None, None),
        Ok(expected)
    );
    assert!(representative_measurement_settings(Some("fixed_mean_32"), Some(21), None).is_err());
    assert!(representative_measurement_settings(Some("fixed_mean_32"), None, Some(31)).is_err());
    assert!(representative_measurement_settings(Some("tail"), Some(20), Some(32)).is_err());
    assert!(representative_measurement_settings(Some("unknown"), None, None).is_err());
    assert_eq!(
        representative_measurement_settings(None, None, None),
        Ok(RepresentativeMeasurementSettings {
            profile: "fixed_mean_32",
            warmup: 20,
            pairs: 32,
        })
    );
    assert!(representative_measurement_settings(None, Some(100), Some(2000)).is_err());
}

#[test]
fn representative_tail_profile_is_withdrawn() {
    assert!(representative_measurement_settings(Some("tail"), None, None).is_err());
    assert!(representative_measurement_settings(Some("tail"), Some(100), Some(2000)).is_err());
}

#[tokio::test]
async fn representative_expired_deadline_never_enters_work() {
    let entered = std::cell::Cell::new(false);
    let result = with_representative_deadline(
        tokio::time::Instant::now() - Duration::from_secs(1),
        async {
            entered.set(true);
            7
        },
    )
    .await;
    assert_eq!(result, Err(()));
    assert!(!entered.get());
}

#[tokio::test]
async fn representative_deadline_bounds_held_work() {
    let result = tokio::time::timeout(
        Duration::from_millis(250),
        with_representative_deadline(
            tokio::time::Instant::now() + Duration::from_millis(20),
            std::future::pending::<()>(),
        ),
    )
    .await;
    assert!(
        matches!(result, Ok(Err(()))),
        "held work must meet the inner budget"
    );
}

#[tokio::test]
async fn representative_deadline_refuses_late_synchronous_completion() {
    let result = with_representative_deadline(
        tokio::time::Instant::now() + Duration::from_millis(1),
        async { std::thread::sleep(Duration::from_millis(20)) },
    )
    .await;
    assert_eq!(result, Err(()));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Workload {
    FreshAdmission,
    ContinuingSegment,
    IndividualFencedTools,
}
impl Workload {
    fn has_prefix(self) -> bool {
        self != Self::FreshAdmission
    }
    fn model_total(self) -> usize {
        if self == Self::ContinuingSegment {
            3
        } else {
            2
        }
    }
}

struct RepresentativeTools {
    inner: FileTools,
    capture: bool,
    context: Mutex<Option<ToolDispatchContext>>,
    calls: Mutex<Vec<String>>,
}
#[async_trait]
impl AgentToolDispatcher for RepresentativeTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("resolved root dispatch is required")
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        assert_eq!(
            context.work_authorization().is_some(),
            self.inner.mode == HostMode::LocalGoverned
        );
        if self.capture {
            let mut saved = self.context.lock().expect("context recorder");
            if saved.is_none() {
                *saved = Some(context.clone());
            }
        }
        let outcome = self
            .inner
            .dispatch_resolved_with_context(call, context, plan)
            .await?;
        self.calls
            .lock()
            .expect("effect recorder")
            .push(call.id.to_owned());
        Ok(outcome)
    }
}

struct RepresentativeProvider {
    mode: HostMode,
    workload: Workload,
    markers: Vec<String>,
    invocations: AtomicUsize,
    requests: AtomicUsize,
    barrier: Notify,
    release: Notify,
    // The provider owns the timestamp immediately after the barrier and before
    // any suffix preparation. Harness inspection never occurs inside this span.
    suffix_start: Mutex<Option<Instant>>,
    queue_control: bool,
    initial_barrier: Notify,
    initial_release: Notify,
}
impl RepresentativeProvider {
    fn authorize(&self, request: &PreparedLlmRequest) {
        match self.mode {
            HostMode::TrustedHost => assert!(request.authorization().is_none()),
            HostMode::LocalGoverned => {
                let prepared = request
                    .authorization()
                    .expect("real native companion")
                    .prepare(model_facts())
                    .expect("actual controller preparation");
                let current = prepared.current().expect("current before Entry");
                current.observe_entry().expect("native Entry");
                let current = current
                    .current()
                    .expect("current before scripted transport");
                current
                    .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
                    .expect("native Outcome");
            }
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
    }
    fn successful_feedback(&self, request: &PreparedLlmRequest, ids: impl Iterator<Item = String>) {
        for id in ids {
            let result = feedback(&request.request().messages, &id);
            assert!(!result.is_error);
            assert_eq!(result.text_content(), "record-7 value");
            assert!(result.settlement_failures.is_empty());
        }
    }
}
#[async_trait]
impl LlmClient for RepresentativeProvider {
    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ENDPOINT, MODEL)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("prepared adapter required")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let index = self.invocations.fetch_add(1, Ordering::Relaxed);
        assert!(
            index < self.workload.model_total(),
            "retry/compaction/extra request is not this workload"
        );
        Box::pin(
            futures::stream::once(async move {
                if index == 0 {
                    let text = request
                        .request()
                        .messages
                        .iter()
                        .filter_map(|m| match m {
                            Message::User(user) => Some(user.text_content()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    for (i, marker) in self.markers.iter().enumerate() {
                        assert_eq!(
                            text.contains(marker),
                            !self.queue_control || i == 0,
                            "only actual selected originals reach the model"
                        );
                    }
                    if self.queue_control {
                        self.initial_barrier.notify_one();
                        self.initial_release.notified().await;
                    }
                } else if self.workload.has_prefix() && index == 1 {
                    self.successful_feedback(
                        request,
                        (0..PREFIX_READS).map(|i| format!("prefix-{i}")),
                    );
                    self.barrier.notify_one();
                    self.release.notified().await;
                    if self.workload == Workload::ContinuingSegment {
                        *self.suffix_start.lock().expect("suffix clock") = Some(Instant::now());
                    }
                } else {
                    self.successful_feedback(request, READ_CALLS.into_iter().map(str::to_owned));
                }
                self.authorize(request);
                let reads = if index == 0 && self.workload.has_prefix() {
                    (0..PREFIX_READS)
                        .map(|i| format!("prefix-{i}"))
                        .collect::<Vec<_>>()
                } else if index == 0 || (index == 1 && self.workload == Workload::ContinuingSegment)
                {
                    READ_CALLS.into_iter().map(str::to_owned).collect()
                } else {
                    Vec::new()
                };
                let mut events: Vec<Result<LlmEvent, LlmError>> =
                    reads.iter().map(|id| tool(id, "read_record")).collect();
                if reads.is_empty() {
                    events.push(Ok(LlmEvent::TextDelta {
                        delta: "representative completed".into(),
                        meta: None,
                    }));
                    events.push(done(StopReason::EndTurn));
                } else {
                    events.push(done(StopReason::ToolUse));
                }
                futures::stream::iter(events)
            })
            .flatten(),
        )
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct RepresentativeFixture {
    mode: HostMode,
    machine: Arc<MeerkatMachine>,
    service: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
    provider: Arc<RepresentativeProvider>,
    tools: Arc<RepresentativeTools>,
    session: SessionId,
    inputs: Vec<Input>,
    old_ids: Vec<InputId>,
    grants: Option<Arc<LocalGrantAuthority>>,
    operations: Vec<Vec<GrantLineageRef>>,
}
impl RepresentativeFixture {
    async fn new(mode: HostMode, depth: usize, workload: Workload, path: PathBuf) -> Self {
        Self::new_inner(mode, depth, workload, path, false).await
    }
    async fn new_inner(
        mode: HostMode,
        depth: usize,
        workload: Workload,
        path: PathBuf,
        queue_control: bool,
    ) -> Self {
        let (machine, grants, controller, operations) = if mode == HostMode::LocalGoverned {
            let grants = Arc::new(
                LocalGrantAuthority::new(
                    LocalGrantConfiguration {
                        root: principal("grant-owner"),
                        namespace: id("native-grants"),
                        generation: 1,
                    },
                    LocalAuthorizationPublication::new(),
                    Arc::new(HostAuthorizationClock),
                )
                .unwrap(),
            );
            let controller = issue_lineage(&grants, "controller", "infer", depth);
            let operations = (0..CONTRIBUTORS)
                .map(|i| issue_lineage(&grants, &format!("read-{i}"), "read", depth))
                .collect::<Vec<_>>();
            let expected_controller = controller.clone();
            let expected_operations = operations.clone();
            let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
                let c = claimed.candidate();
                if current.requester() != &principal("requester")
                    || current.ingress_actor() != &principal("ingress")
                    || current.realm() != &RealmId::parse("native-loop").unwrap()
                    || c.requester != principal("requester")
                    || c.logical_executor != principal("executor")
                    || c.represented_subject.is_some()
                    || c.target.logical_runtime != id(&runtime.to_string())
                    || c.controller_grant_lineage != expected_controller
                    || c.controller_model.as_ref() != Some(&selection())
                    || c.controller_ceiling != ceiling("infer")
                    || !expected_operations.iter().any(|chain| {
                        c.authority_basis
                            == WorkAuthorityBasis::GrantLineage {
                                lineage: chain.clone(),
                            }
                    })
                {
                    return Err(denied().into());
                }
                Ok(())
            });
            let machine = MeerkatMachine::ephemeral()
                .with_local_grant_authorization(NativeGrantWorkConfiguration {
                    grants: grants.clone(),
                    ingress,
                    invocation_owner: Arc::new(InvocationOwner),
                    operation_owner: Arc::new(RecordOwner),
                })
                .unwrap();
            (machine, Some(grants), controller, operations)
        } else {
            (
                MeerkatMachine::ephemeral(),
                None,
                Vec::new(),
                vec![Vec::new(); CONTRIBUTORS],
            )
        };
        let machine = Arc::new(machine);
        let markers = (0..CONTRIBUTORS)
            .map(|i| format!("representative-original-{i}"))
            .collect::<Vec<_>>();
        let provider = Arc::new(RepresentativeProvider {
            mode,
            workload,
            markers: markers.clone(),
            invocations: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            barrier: Notify::new(),
            release: Notify::new(),
            suffix_start: Mutex::new(None),
            queue_control,
            initial_barrier: Notify::new(),
            initial_release: Notify::new(),
        });
        let tools = Arc::new(RepresentativeTools {
            inner: FileTools {
                path,
                mode,
                script: Script::Allowed,
                reads: AtomicUsize::new(0),
                deletes: AtomicUsize::new(0),
                before_read: Notify::new(),
                release_read: Notify::new(),
            },
            capture: workload == Workload::IndividualFencedTools,
            context: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
        });
        let mut config = Config::default();
        config.agent.max_turns = Some(4);
        config.budget.max_tool_calls = Some(400);
        config.compaction.auto_compact_threshold = 1_000_000;
        config.compaction.auto_compact_threshold_explicit = true;
        config.compaction.max_request_bytes = Some(16_000_000);
        let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), config);
        builder.default_llm_client = Some(provider.clone());
        builder.default_tool_dispatcher = Some(tools.clone());
        builder.default_session_store = Some(Arc::new(SessionStore::default()));
        let service = Arc::new(EphemeralSessionService::new(builder, 2));
        let created = meerkat::surface::materialize_ephemeral_runtime_session(
            &service,
            &machine,
            CreateSessionRequest {
                model: MODEL.into(),
                prompt: "".into(),
                injected_context: Vec::new(),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            },
            false,
        )
        .await
        .unwrap();
        let session = created.session_id;
        let actor = service.live_session_actor_witness(&session).await.unwrap();
        let pin = service
            .pin_controller_client_for_actor(&actor)
            .await
            .unwrap();
        assert_eq!(pin.selection(), &selection());
        let owner = machine.generated_auth_lease_handle();
        meerkat_core::publish_token_lifecycle_acquired_for_identity(
            &owner,
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-cost-credential"),
        )
        .unwrap();
        assert_eq!(
            owner
                .resolve_credential_use_admission(
                    &meerkat_core::handles::LeaseKey::from_credential_identity(
                        pin.selection().credential()
                    ),
                    meerkat_core::handles::CredentialUseIntent::HoldAuthority
                )
                .unwrap(),
            meerkat_core::handles::CredentialUseDisposition::Authorized
        );
        let make_input = |marker: &str, index: usize| {
            let mut prompt = PromptInput::new(
                marker,
                Some(RuntimeTurnMetadata {
                    handling_mode: Some(if queue_control {
                        HandlingMode::Queue
                    } else {
                        HandlingMode::Steer
                    }),
                    ..Default::default()
                }),
            );
            if mode == HostMode::LocalGoverned {
                let mut c = association(
                    &LogicalRuntimeId::for_session(&session),
                    controller.clone(),
                    operations[index % CONTRIBUTORS].clone(),
                    pin.selection().clone(),
                )
                .candidate()
                .clone();
                c.original_work.work = id(marker);
                c.original_authentication = evidence(&format!("auth-{marker}"));
                c.root_event = evidence(&format!("event-{marker}"));
                prompt.header.authority_association =
                    Some(InputAuthorityAssociation::new(c).unwrap());
            }
            let mut input = Input::Prompt(prompt);
            if mode == HostMode::LocalGoverned {
                let ingress = NativeIngressContext::from_trusted_ingress(
                    &input,
                    principal("requester"),
                    principal("ingress"),
                    RealmId::parse("native-loop").unwrap(),
                    evidence(&format!("current-{marker}")),
                )
                .unwrap()
                .with_controller_client(&input, pin.clone())
                .unwrap();
                input = input.with_ingress_context(ingress).unwrap();
            }
            input
        };
        let mut old_ids = Vec::with_capacity(OLD_ROWS);
        for i in 0..OLD_ROWS {
            let input = make_input(&format!("old-cancelled-{i}"), i);
            let input_id = input.id().clone();
            assert!(
                machine
                    .accept_input_without_wake(&session, input)
                    .await
                    .unwrap()
                    .is_accepted()
            );
            assert!(
                machine
                    .cancel_input_if_present(
                        &session,
                        &input_id,
                        "cost fixture queued cancellation"
                    )
                    .await
                    .unwrap()
            );
            let row = machine
                .input_state(&session, &input_id)
                .await
                .unwrap()
                .expect("actual retained old row");
            assert_eq!(
                row.seed.terminal_outcome,
                Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled
                })
            );
            assert!(row.seed.last_run_id.is_none());
            old_ids.push(input_id);
        }
        assert!(
            machine
                .list_active_inputs(&session)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(provider.requests.load(Ordering::Relaxed), 0);
        assert_eq!(tools.inner.reads.load(Ordering::Relaxed), 0);
        let inputs = markers
            .iter()
            .enumerate()
            .map(|(i, marker)| make_input(marker, i))
            .collect();
        Self {
            mode,
            machine,
            service,
            provider,
            tools,
            session,
            inputs,
            old_ids,
            grants,
            operations,
        }
    }
    async fn start(&self) -> meerkat_runtime::completion::CompletionHandle {
        for input in &self.inputs[..CONTRIBUTORS - 1] {
            assert!(
                self.machine
                    .accept_input_without_wake(&self.session, input.clone())
                    .await
                    .unwrap()
                    .is_accepted()
            );
        }
        let (accepted, completion) = self
            .machine
            .accept_input_with_completion(&self.session, self.inputs[CONTRIBUTORS - 1].clone())
            .await
            .unwrap();
        assert!(accepted.is_accepted());
        completion.expect("real fourth-input completion handle")
    }
    async fn audit(&self) -> (RunId, Vec<StoredAuthorizationAuditObservation>) {
        let mut run = None;
        let mut found = Vec::new();
        for input in &self.inputs {
            let row = self
                .machine
                .input_state(&self.session, input.id())
                .await
                .unwrap()
                .expect("actual contributor row");
            let current = row
                .seed
                .last_run_id
                .clone()
                .expect("every original staged on actual run");
            if let Some(run) = &run {
                assert_eq!(run, &current);
            } else {
                run = Some(current);
            }
            let wire = serde_json::to_value(row).unwrap();
            if let Some(records) = wire.get("authorization_audit") {
                let records: Vec<StoredAuthorizationAuditObservation> =
                    serde_json::from_value(records.clone()).unwrap();
                if !records.is_empty() {
                    assert!(
                        found.is_empty(),
                        "one canonical native row owns this buffer"
                    );
                    found = records;
                }
            }
        }
        let run = run.unwrap();
        let expected: BTreeSet<_> = self.inputs.iter().map(|i| i.id().to_string()).collect();
        assert_eq!(expected.len(), CONTRIBUTORS);
        for record in &found {
            assert_eq!(
                record
                    .contributors
                    .iter()
                    .map(|i| i.input_id.to_string())
                    .collect::<BTreeSet<_>>(),
                expected
            );
            assert_eq!(record.contributors.len(), CONTRIBUTORS);
            assert_eq!(record.observation.run_id.as_ref(), Some(&run));
            for c in record.contributors.iter() {
                assert_eq!(c.requester, principal("requester"));
                assert_eq!(c.logical_executor, principal("executor"));
                assert!(c.represented_subject.is_none());
            }
            assert!(
                matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput { owner_session_id, canonical_input_id, submitted_input_id, .. }
                if owner_session_id == &self.session && expected.contains(&canonical_input_id.to_string()) && submitted_input_id == canonical_input_id)
            );
        }
        if self.mode == HostMode::TrustedHost {
            assert!(found.is_empty());
        }
        (run, found)
    }
    async fn retained_old_rows(&self) {
        for id in &self.old_ids {
            let row = self
                .machine
                .input_state(&self.session, id)
                .await
                .unwrap()
                .expect("retained old row after run");
            assert_eq!(
                row.seed.terminal_outcome,
                Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled
                })
            );
        }
    }
    async fn close(&self) {
        self.machine.retire_runtime(&self.session).await.unwrap();
        self.service.try_shutdown().await.unwrap();
    }
    fn context(&self) -> ToolDispatchContext {
        let captured = self
            .tools
            .context
            .lock()
            .unwrap()
            .clone()
            .expect("actual entered prefix call context");
        assert_eq!(
            captured.work_authorization().is_some(),
            self.mode == HostMode::LocalGoverned
        );
        captured
            .clone()
            .with_work_authorization(captured.work_authorization().cloned())
    }
}

fn assert_success_audit(
    audit: &[StoredAuthorizationAuditObservation],
    operations: usize,
    mode: HostMode,
) {
    if mode == HostMode::TrustedHost {
        assert!(audit.is_empty());
        return;
    }
    assert_eq!(audit.len(), operations * 3);
    let prepared = audit
        .iter()
        .filter(|r| matches!(r.observation.observation, AuditObservation::Prepared { .. }))
        .collect::<Vec<_>>();
    assert_eq!(prepared.len(), operations);
    assert_eq!(
        prepared
            .iter()
            .map(|r| r.observation.operation_id.to_string())
            .collect::<BTreeSet<_>>()
            .len(),
        operations
    );
    let mut records_by_operation: HashMap<_, Vec<_>> = HashMap::new();
    for record in audit {
        records_by_operation
            .entry(record.observation.operation_id.clone())
            .or_default()
            .push(record);
    }
    for p in prepared {
        let records = records_by_operation
            .get(&p.observation.operation_id)
            .expect("every prepared observation has an indexed operation");
        assert_eq!(records.len(), 3);
        assert!(matches!(
            records[0].observation.observation,
            AuditObservation::Prepared { .. }
        ));
        assert!(matches!(
            records[1].observation.observation,
            AuditObservation::Entry
        ));
        let AuditObservation::Prepared { target, .. } = &p.observation.observation else {
            unreachable!("selected Prepared record")
        };
        assert!(
            matches!(
                (target.as_ref(), &records[2].observation.observation),
                (
                    AuditTarget::Model(_),
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                    }
                ) | (
                    AuditTarget::Tool { .. },
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchReturned {
                            result_is_error: false,
                            terminal_error: None,
                            ..
                        }
                    }
                )
            ),
            "the observed outcome must match its prepared target class"
        );
    }
}

async fn direct_call(
    f: &RepresentativeFixture,
    context: &ToolDispatchContext,
    call_id: &str,
) -> (u64, Result<ToolDispatchOutcome, ToolError>) {
    let arguments =
        serde_json::value::RawValue::from_string(r#"{"record":"record-7"}"#.into()).unwrap();
    let call = ToolCallView {
        id: call_id,
        name: "read_record",
        args: &arguments,
    };
    let start = Instant::now();
    let deadlines =
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_secs(30),
        )])
        .unwrap();
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(deadlines);
    let result = match meerkat_core::resolve_tool_execution_plan_fenced(
        &f.tools,
        call,
        context,
        &resolution,
    ) {
        Ok(plan) => match f
            .tools
            .validate_resolved_execution_plan(call, &resolution, &plan)
        {
            Ok(()) => {
                meerkat_core::dispatch_tool_execution_plan_fenced(&f.tools, call, context, &plan)
                    .await
            }
            Err(error) => Err(error.into()),
        },
        Err(error) => Err(error.into()),
    };
    (ns(start.elapsed()), result)
}

#[derive(Serialize)]
struct RepresentativeSample {
    workload: Workload,
    pair: usize,
    first_in_pair: bool,
    depth: usize,
    mode: HostMode,
    measured_ns: Option<u64>,
    individual_ns: Vec<(String, u64)>,
    old_rows: usize,
    active_rows: usize,
    total_rows: usize,
    contributor_ids: Vec<String>,
    run_id: String,
    prefix_audit_records: usize,
    prefix_audit_digest: EvidenceDigest,
    prefix_reads: usize,
    final_audit_records: usize,
    final_reads: usize,
    model_requests: usize,
    measured_operation_ids: Vec<String>,
    measured_call_ids: Vec<String>,
}
async fn representative_sample(
    mode: HostMode,
    depth: usize,
    workload: Workload,
    path: PathBuf,
    pair: usize,
    first_in_pair: bool,
) -> RepresentativeSample {
    let f = RepresentativeFixture::new(mode, depth, workload, path).await;
    let fresh_start = (workload == Workload::FreshAdmission).then(Instant::now);
    let completion = f.start().await;
    let mut prefix = Vec::new();
    let mut individual_ns = Vec::new();
    let mut prior_run = None;
    if workload.has_prefix() {
        tokio::time::timeout(Duration::from_secs(30), f.provider.barrier.notified())
            .await
            .unwrap();
        assert_eq!(
            f.provider.requests.load(Ordering::Relaxed),
            1,
            "barrier precedes suffix authorization"
        );
        assert_eq!(f.tools.inner.reads.load(Ordering::Relaxed), PREFIX_READS);
        let observed_prefix = f.tools.calls.lock().unwrap().clone();
        assert_eq!(observed_prefix.len(), PREFIX_READS);
        assert_eq!(
            observed_prefix.into_iter().collect::<BTreeSet<_>>(),
            (0..PREFIX_READS)
                .map(|i| format!("prefix-{i}"))
                .collect::<BTreeSet<_>>(),
            "the completed physical prefix must have the exact expected call IDs"
        );
        let (run, audit) = f.audit().await;
        prior_run = Some(run);
        assert_success_audit(&audit, PREFIX_READS + 1, mode);
        assert_eq!(
            audit.len(),
            if mode == HostMode::LocalGoverned {
                PREFIX_RECORDS
            } else {
                0
            }
        );
        prefix = audit;
        if workload == Workload::IndividualFencedTools {
            let context = f.context();
            for i in 0..4 {
                let call = format!("direct-{i}");
                let (elapsed, result) = direct_call(&f, &context, &call).await;
                let result = result.expect("actual fenced read succeeds");
                assert!(!result.result.is_error && result.result.settlement_failures.is_empty());
                assert!(
                    result.async_ops.is_empty()
                        && result.session_effects.is_empty()
                        && result.terminal_cause().is_none()
                );
                assert_eq!(result.result.tool_use_id, call);
                individual_ns.push((call, elapsed));
            }
        }
        f.provider.release.notify_one();
    }
    let outcome = completion.wait().await.expect("native completion");
    let completed_at = Instant::now();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("normal same-run completion required")
    };
    assert_eq!(result.text, "representative completed");
    assert_eq!(result.session_id, f.session);
    assert!(result.terminal_cause_kind.is_none());
    assert_eq!(result.turns as usize, workload.model_total());
    let direct = workload == Workload::IndividualFencedTools;
    assert_eq!(
        result.tool_calls as usize,
        if direct {
            PREFIX_READS
        } else if workload.has_prefix() {
            PREFIX_READS + 4
        } else {
            4
        }
    );
    let measured_ns = match workload {
        Workload::FreshAdmission => Some(ns(completed_at.duration_since(fresh_start.unwrap()))),
        Workload::ContinuingSegment => Some(ns(
            completed_at.duration_since(f.provider.suffix_start.lock().unwrap().unwrap())
        )),
        Workload::IndividualFencedTools => None,
    };
    let (run, audit) = f.audit().await;
    if let Some(prior) = prior_run {
        assert_eq!(prior, run);
    }
    assert!(
        audit.starts_with(&prefix),
        "immutable native audit prefix/order retained"
    );
    let expected_reads = if workload.has_prefix() {
        PREFIX_READS + 4
    } else {
        4
    };
    assert_eq!(f.tools.inner.reads.load(Ordering::Relaxed), expected_reads);
    assert_eq!(f.tools.inner.deletes.load(Ordering::Relaxed), 0);
    assert_eq!(
        f.provider.requests.load(Ordering::Relaxed),
        workload.model_total()
    );
    assert_success_audit(&audit, expected_reads + workload.model_total(), mode);
    let calls = f.tools.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), expected_reads);
    assert_eq!(calls.iter().collect::<BTreeSet<_>>().len(), expected_reads);
    let expected_suffix: BTreeSet<String> = if direct {
        (0..4).map(|i| format!("direct-{i}")).collect()
    } else {
        READ_CALLS.into_iter().map(str::to_owned).collect()
    };
    let expected_prefix: BTreeSet<String> = if workload.has_prefix() {
        (0..PREFIX_READS).map(|i| format!("prefix-{i}")).collect()
    } else {
        BTreeSet::new()
    };
    let observed_calls: BTreeSet<_> = calls.into_iter().collect();
    assert_eq!(
        observed_calls,
        expected_prefix
            .union(&expected_suffix)
            .cloned()
            .collect::<BTreeSet<_>>(),
        "all completed physical calls must be the exact prefix and suffix"
    );
    let measured_call_ids: Vec<_> = observed_calls
        .difference(&expected_prefix)
        .cloned()
        .collect();
    assert_eq!(
        measured_call_ids.iter().cloned().collect::<BTreeSet<_>>(),
        expected_suffix
    );
    let mut audited_suffix_calls = Vec::new();
    let mut measured_operation_ids = Vec::new();
    for record in &audit[prefix.len()..] {
        let AuditObservation::Prepared { target, .. } = &record.observation.observation else {
            continue;
        };
        let is_tool = match target.as_ref() {
            AuditTarget::Tool {
                call_id, tool_name, ..
            } => {
                assert_eq!(tool_name, "read_record");
                audited_suffix_calls.push(call_id.clone());
                true
            }
            AuditTarget::Model(_) => false,
            _ => panic!("unexpected prepared suffix target"),
        };
        if !direct || is_tool {
            measured_operation_ids.push(record.observation.operation_id.to_string());
        }
    }
    if mode == HostMode::LocalGoverned {
        assert_eq!(audited_suffix_calls.len(), measured_call_ids.len());
        assert_eq!(
            audited_suffix_calls.into_iter().collect::<BTreeSet<_>>(),
            measured_call_ids.iter().cloned().collect::<BTreeSet<_>>(),
            "actual prepared tool IDs must join the observed physical suffix"
        );
    } else {
        assert!(audited_suffix_calls.is_empty());
    }
    assert_eq!(
        measured_operation_ids.len(),
        if mode == HostMode::TrustedHost {
            0
        } else if direct {
            4
        } else {
            6
        }
    );
    for input in &f.inputs {
        let row = f
            .machine
            .input_state(&f.session, input.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
    }
    f.retained_old_rows().await;
    assert_eq!(std::fs::read(&f.tools.inner.path).unwrap(), FILE_BYTES);
    let value = RepresentativeSample {
        workload,
        pair,
        first_in_pair,
        depth,
        mode,
        measured_ns,
        individual_ns,
        old_rows: f.old_ids.len(),
        active_rows: f.inputs.len(),
        total_rows: f.old_ids.len() + f.inputs.len(),
        contributor_ids: f.inputs.iter().map(|i| i.id().to_string()).collect(),
        run_id: run.to_string(),
        prefix_audit_records: prefix.len(),
        prefix_audit_digest: EvidenceDigest::of_bytes(&serde_json::to_vec(&prefix).unwrap()),
        prefix_reads: if workload.has_prefix() {
            PREFIX_READS
        } else {
            0
        },
        final_audit_records: audit.len(),
        final_reads: expected_reads,
        model_requests: workload.model_total(),
        measured_operation_ids,
        measured_call_ids,
    };
    f.close().await;
    value
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_representative_correctness() {
    let path = fixture_file();
    for depth in [1, 3] {
        for workload in [
            Workload::FreshAdmission,
            Workload::ContinuingSegment,
            Workload::IndividualFencedTools,
        ] {
            for mode in [HostMode::TrustedHost, HostMode::LocalGoverned] {
                tokio::time::timeout(
                    Duration::from_secs(60),
                    representative_sample(mode, depth, workload, path.clone(), 0, true),
                )
                .await
                .unwrap();
            }
        }
    }
    // Queue is a separate canonical selector, inspected at the first provider
    // boundary. Only its first original may have a run before the others cancel.
    for mode in [HostMode::TrustedHost, HostMode::LocalGoverned] {
        let f =
            RepresentativeFixture::new_inner(mode, 1, Workload::FreshAdmission, path.clone(), true)
                .await;
        // Retain each waiter's real first admission; these inputs have no replay key.
        let mut completions = Vec::with_capacity(CONTRIBUTORS);
        for input in &f.inputs {
            let (accepted, completion) = f
                .machine
                .accept_input_with_completion(&f.session, input.clone())
                .await
                .unwrap();
            assert!(accepted.is_accepted());
            completions.push(completion.expect("real queue-input completion handle"));
        }
        tokio::time::timeout(
            Duration::from_secs(30),
            f.provider.initial_barrier.notified(),
        )
        .await
        .unwrap();
        let first = f
            .machine
            .input_state(&f.session, f.inputs[0].id())
            .await
            .unwrap()
            .unwrap();
        assert!(first.seed.last_run_id.is_some());
        for input in &f.inputs[1..] {
            let row = f
                .machine
                .input_state(&f.session, input.id())
                .await
                .unwrap()
                .unwrap();
            assert!(row.seed.last_run_id.is_none());
            assert!(
                f.machine
                    .cancel_input_if_present(&f.session, input.id(), "Queue control cleanup")
                    .await
                    .unwrap()
            );
        }
        let mut completions = completions.into_iter();
        let first_completion = completions.next().expect("first queue-input completion");
        f.provider.initial_release.notify_one();
        let first_outcome = first_completion.wait().await.unwrap();
        assert!(
            matches!(&first_outcome, CompletionOutcome::Completed(_)),
            "expected first Queue input to complete, got {first_outcome:?}"
        );
        for (input, completion) in f.inputs[1..].iter().zip(completions) {
            let outcome = completion.wait().await.unwrap();
            assert!(
                matches!(
                    &outcome,
                    CompletionOutcome::RuntimeTerminated { reason, error }
                        if reason == "Queue control cleanup"
                            && error == &meerkat_core::TurnErrorMetadata::terminal(
                                meerkat_core::TurnTerminalCauseKind::FatalFailure,
                                meerkat_core::TurnTerminalOutcome::Failed,
                                "Queue control cleanup",
                            )
                ),
                "expected exact runless Queue cancellation completion, got {outcome:?}"
            );
            let row = f
                .machine
                .input_state(&f.session, input.id())
                .await
                .unwrap()
                .expect("cancelled Queue input retains its actual row");
            assert!(
                matches!(
                    &row.seed.terminal_outcome,
                    Some(InputTerminalOutcome::Abandoned {
                        reason: InputAbandonReason::Cancelled,
                    })
                ),
                "expected queued Abandoned(Cancelled), got {:?}",
                row.seed.terminal_outcome
            );
            assert!(
                row.seed.last_run_id.is_none(),
                "cancelled Queue input must never have a run, got {:?}",
                row.seed.last_run_id
            );
        }
        assert_eq!(f.provider.requests.load(Ordering::Relaxed), 2);
        assert_eq!(f.tools.inner.reads.load(Ordering::Relaxed), 4);
        assert_eq!(f.tools.inner.deletes.load(Ordering::Relaxed), 0);
        f.close().await;
    }
    // A freshly authenticated but wrong requester/target cannot enter the
    // native owner. This is an admission control, not a fake batch carrier.
    for wrong_requester in [false, true] {
        let f = RepresentativeFixture::new(
            HostMode::LocalGoverned,
            1,
            Workload::FreshAdmission,
            path.clone(),
        )
        .await;
        let original = &f.inputs[0];
        let pin = original
            .header()
            .ingress_context
            .as_ref()
            .unwrap()
            .controller_client()
            .unwrap()
            .clone();
        let mut c = original
            .header()
            .authority_association
            .as_ref()
            .unwrap()
            .candidate()
            .clone();
        let requester = if wrong_requester {
            principal("different-requester")
        } else {
            principal("requester")
        };
        c.requester = requester.clone();
        if !wrong_requester {
            c.target.logical_runtime = id("different-native-target");
        }
        let mut prompt = PromptInput::new(
            "wrong-owner-input",
            Some(RuntimeTurnMetadata {
                handling_mode: Some(HandlingMode::Steer),
                ..Default::default()
            }),
        );
        prompt.header.authority_association = Some(InputAuthorityAssociation::new(c).unwrap());
        let input = Input::Prompt(prompt);
        let current = NativeIngressContext::from_trusted_ingress(
            &input,
            requester,
            principal("ingress"),
            RealmId::parse("native-loop").unwrap(),
            evidence("fresh-negative-ingress"),
        )
        .unwrap()
        .with_controller_client(&input, pin)
        .unwrap();
        let input = input.with_ingress_context(current).unwrap();
        let id = input.id().clone();
        let result = f.machine.accept_input_without_wake(&f.session, input).await;
        if wrong_requester {
            assert!(matches!(
                result,
                Err(meerkat_runtime::RuntimeDriverError::InputRefused { refusal })
                    if refusal.kind() == OperationRefusalKind::Denied
            ));
        } else {
            assert!(matches!(
                result,
                Err(meerkat_runtime::RuntimeDriverError::ValidationFailed { reason })
                    if reason == "native work authority association is unavailable or mismatched"
            ));
        }
        assert!(
            f.machine
                .input_state(&f.session, &id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(f.provider.requests.load(Ordering::Relaxed), 0);
        assert_eq!(f.tools.inner.reads.load(Ordering::Relaxed), 0);
        f.close().await;
    }
    // Real revoked-contributor and ended-run controls use the same captured
    // context/public root helpers. No actor, audit or grant substitute exists.
    for revoke in [false, true] {
        let f = RepresentativeFixture::new(
            HostMode::LocalGoverned,
            3,
            Workload::IndividualFencedTools,
            path.clone(),
        )
        .await;
        let completion = f.start().await;
        tokio::time::timeout(Duration::from_secs(30), f.provider.barrier.notified())
            .await
            .unwrap();
        let (run, before) = f.audit().await;
        assert_success_audit(&before, PREFIX_READS + 1, f.mode);
        let context = f.context();
        if revoke {
            let mut custody = f.machine.try_controller_grant_mutation().unwrap();
            f.grants
                .as_ref()
                .unwrap()
                .revoke(
                    &principal("grant-owner"),
                    f.operations[2].last().unwrap(),
                    &mut custody,
                )
                .unwrap();
            drop(custody);
            let (_, result) = direct_call(&f, &context, "revoked-contributor-read").await;
            assert!(matches!(
                result,
                Err(ToolError::AuthorizationRefused { .. })
            ));
            assert_eq!(f.tools.inner.reads.load(Ordering::Relaxed), PREFIX_READS);
            let (_, after) = f.audit().await;
            assert!(after.starts_with(&before));
            let refusal = after.iter().find(|r| matches!(&r.observation.observation, AuditObservation::Refused { target, .. }
                if matches!(target.as_ref(), AuditTarget::Tool { call_id, .. } if call_id == "revoked-contributor-read"))).expect("exact attempted call refused");
            assert!(
                !after.iter().any(|r| r.observation.operation_id
                    == refusal.observation.operation_id
                    && matches!(
                        r.observation.observation,
                        AuditObservation::Entry | AuditObservation::Outcome { .. }
                    )),
                "denied preparation never entered"
            );
        }
        f.provider.release.notify_one();
        let result = completion.wait().await.unwrap();
        assert!(
            matches!(result, CompletionOutcome::Completed(_)),
            "independent controller still finishes same run"
        );
        assert_eq!(f.audit().await.0, run);
        let (_, stale) = direct_call(&f, &context, "ended-run-read").await;
        assert!(matches!(stale, Err(ToolError::AuthorizationRefused { .. })));
        assert_eq!(f.tools.inner.reads.load(Ordering::Relaxed), PREFIX_READS);
        assert_eq!(f.tools.inner.deletes.load(Ordering::Relaxed), 0);
        f.close().await;
    }
    std::fs::remove_file(path).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit quiet-host performance lease; full representative setup is expensive"]
async fn native_representative_matrix() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1200);
    assert_eq!(
        std::env::var("NATIVE_COST_RUN").as_deref(),
        Ok("approved-quiet-window")
    );
    assert!(!std::hint::black_box(cfg!(debug_assertions)));
    let count = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.parse::<usize>().unwrap())
    };
    let profile = std::env::var("NATIVE_COST_MEASUREMENT_PROFILE").ok();
    let settings = representative_measurement_settings(
        profile.as_deref(),
        count("NATIVE_COST_WARMUP_PAIRS"),
        count("NATIVE_COST_PAIRS"),
    )
    .expect("valid declared representative measurement profile");
    let warmup = settings.warmup;
    let pairs = settings.pairs;
    let output_path =
        std::env::var_os("NATIVE_COST_OUTPUT").expect("fresh measurement output path");
    let output_created = std::cell::Cell::new(false);
    let completed = with_representative_deadline(deadline, async {
        let path = fixture_file();
        let mut samples = Vec::new();
        for depth in [1, 3] {
            for workload in [
                Workload::FreshAdmission,
                Workload::ContinuingSegment,
                Workload::IndividualFencedTools,
            ] {
                for iteration in 0..warmup + pairs {
                    let order = if iteration % 2 == 0 {
                        [HostMode::TrustedHost, HostMode::LocalGoverned]
                    } else {
                        [HostMode::LocalGoverned, HostMode::TrustedHost]
                    };
                    for (position, mode) in order.into_iter().enumerate() {
                        let value = tokio::time::timeout(
                            Duration::from_secs(60),
                            representative_sample(
                                mode,
                                depth,
                                workload,
                                path.clone(),
                                iteration.saturating_sub(warmup),
                                position == 0,
                            ),
                        )
                        .await
                        .expect("failed/slow samples fail the run; never discard them");
                        if iteration >= warmup {
                            samples.push(value);
                        }
                    }
                }
            }
        }
        std::fs::remove_file(path).unwrap();
        let payload = serde_json::json!({"schema":2,"suite":"representative","measurement_profile":settings.profile,"measurement_status":"complete","samples":samples,"warmup_pairs":warmup,"pairs_per_cell":pairs,
            "failures":0,"timeouts":0,"acceptance":"not evaluated","scope":"fresh admission and continuing segment separated; direct fenced tool excludes Agent scheduling"});
        use std::io::Write;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .unwrap();
        output_created.set(true);
        output
            .write_all(&serde_json::to_vec_pretty(&payload).unwrap())
            .unwrap();
    })
    .await;
    if completed.is_err() && output_created.get() {
        std::fs::remove_file(&output_path).expect("remove only this run's late output");
    }
    assert!(
        completed.is_ok(),
        "UNCERTAIN: representative overall budget exhausted"
    );
}
