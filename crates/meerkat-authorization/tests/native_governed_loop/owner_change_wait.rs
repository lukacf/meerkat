//! Owner changes in progress on the run's own publication, on the real native
//! loop: the same grant, admission and Agent owners as the parent loop test.
//! Another thread holds each change, as an owner holds one across its storage
//! commit, while the model request prepares.

use super::*;
use meerkat_authorization_contracts::audit::AuditRefusalBasis;
use meerkat_core::OperationId;
use meerkat_core::authorization::{
    OWNER_CHANGE_FREE_REBUILDS, OwnerChangeInProgress, PolicyPublicationObservation,
};

const ANSWER: &str = "answered after the owner change";
const TOOL_CALL: &str = "read-during-change";

/// How the scripted transport churns the run's publication.
#[derive(Clone, Copy)]
enum Churn {
    /// At the first request, one change held for `hold`.
    HeldOnce { hold: Duration },
    /// At every request of the run's first operation, a fresh change held for
    /// `hold`: changes that keep beginning as each one completes. Other
    /// operations find the last one completed.
    EveryRequestOfFirstOperation { hold: Duration },
    /// The first request answers with a tool call and, once that request is
    /// checked, begins one change held for `hold`: the tool's dispatch meets
    /// it. A request carrying the tool's result answers.
    HeldOverToolCall { hold: Duration },
}

/// The host's operation refusal observer: keeps what it is given.
#[derive(Default)]
struct RefusalLog(Mutex<Vec<meerkat_core::OperationRefusalObservation>>);

impl meerkat_core::OperationRefusalObserver for RefusalLog {
    fn observe(&self, observation: &meerkat_core::OperationRefusalObservation) {
        self.0
            .lock()
            .expect("refusal log")
            .push(observation.clone());
    }
}

impl RefusalLog {
    fn observed(&self) -> Vec<meerkat_core::OperationRefusalObservation> {
        self.0.lock().expect("refusal log").clone()
    }
}

/// Starts the hold of a change [`ChurnProvider::hold_change`] began when it
/// is dropped; until then the change stays in progress.
struct HoldStart {
    _start: std::sync::mpsc::Sender<()>,
}

/// One request the transport received.
struct Request {
    operation: OperationId,
    messages: Vec<Message>,
    /// The publication the request's successful preparation observed.
    observed: Option<PolicyPublicationObservation>,
}

struct ChurnProvider {
    churn: Churn,
    publication: LocalAuthorizationPublication,
    holders: Mutex<Vec<std::thread::JoinHandle<()>>>,
    first_held: Mutex<Option<OwnerChangeInProgress>>,
    requests: Mutex<Vec<Request>>,
    ready_to_finish: Notify,
    finish: Notify,
}

impl ChurnProvider {
    fn new(churn: Churn, publication: LocalAuthorizationPublication) -> Self {
        Self {
            churn,
            publication,
            holders: Mutex::new(Vec::new()),
            first_held: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            ready_to_finish: Notify::new(),
            finish: Notify::new(),
        }
    }

    /// Begin an owner change on another thread and return once it is in
    /// progress. The thread keeps the change in progress until the returned
    /// start is dropped, however late the caller runs, then holds it for
    /// `hold` and completes it.
    fn hold_change(&self, hold: Duration) -> HoldStart {
        let (held, in_progress) = std::sync::mpsc::channel();
        let (start, started) = std::sync::mpsc::channel::<()>();
        let publication = self.publication.clone();
        let holder = std::thread::spawn(move || {
            let change = publication
                .begin_owner_change()
                .expect("fixture owner change");
            held.send(()).expect("held change reported");
            started
                .recv()
                .expect_err("nothing is sent: dropping the start starts the hold");
            std::thread::sleep(hold);
            drop(change);
        });
        in_progress.recv().expect("the change is in progress");
        let mut first_held = self.first_held.lock().expect("held token");
        if first_held.is_none() {
            *first_held = self.publication.change_in_progress();
        }
        self.holders.lock().expect("holders").push(holder);
        HoldStart { _start: start }
    }

    /// Wait for every held change to complete.
    fn join_holders(&self) {
        let holders = std::mem::take(&mut *self.holders.lock().expect("holders"));
        for holder in holders {
            holder.join().expect("holder thread");
        }
    }

    fn requests(&self) -> Vec<(OperationId, Option<PolicyPublicationObservation>)> {
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .map(|request| (request.operation.clone(), request.observed))
            .collect()
    }
}

#[async_trait]
impl LlmClient for ChurnProvider {
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
        panic!("governed request must reach the prepared provider entry")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let authorization = request
            .authorization()
            .expect("native work reaches actual provider");
        let operation = authorization.operation_id().clone();
        let first_operation = self
            .requests
            .lock()
            .expect("requests")
            .first()
            .map(|first| first.operation.clone());
        // A change begun here stays in progress until the run takes this
        // request's refusal, so the check below meets it however late this
        // thread runs; its hold starts only then.
        let hold_start = match self.churn {
            Churn::HeldOnce { hold } if first_operation.is_none() => Some(self.hold_change(hold)),
            Churn::EveryRequestOfFirstOperation { hold }
                if first_operation
                    .as_ref()
                    .is_none_or(|first| *first == operation) =>
            {
                Some(self.hold_change(hold))
            }
            // Another operation finds the last change completed.
            Churn::EveryRequestOfFirstOperation { .. } => {
                self.join_holders();
                None
            }
            Churn::HeldOnce { .. } | Churn::HeldOverToolCall { .. } => None,
        };
        // The physical transport's own exact target, checked immediately
        // before entry, as the real adapter does.
        let checked = authorization
            .prepare(model_facts())
            .and_then(|prepared| prepared.current());
        let mut requests = self.requests.lock().expect("requests");
        let check = match checked {
            Ok(check) => check,
            Err(error) => {
                requests.push(Request {
                    operation,
                    messages: request.request().messages.clone(),
                    observed: None,
                });
                return Box::pin(futures::stream::once(async move {
                    // The run takes the refusal: the held change's hold starts.
                    drop(hold_start);
                    Err(LlmError::from_operation_authorization(error))
                }));
            }
        };
        drop(hold_start);
        check
            .observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
            .expect("native audit stages actual transport entry");
        check
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
            .expect("native audit stages fixture response headers");
        requests.push(Request {
            operation,
            messages: request.request().messages.clone(),
            observed: check.policy_observation(),
        });
        if let Churn::HeldOverToolCall { hold } = self.churn
            && tool_feedback(&request.request().messages, TOOL_CALL).is_none()
        {
            return Box::pin(
                futures::stream::once(async move {
                    if self.first_held.lock().expect("held token").is_none() {
                        // The tool's dispatch meets this change: its hold
                        // starts at once.
                        drop(self.hold_change(hold));
                    }
                    Ok(LlmEvent::ToolCallComplete {
                        id: TOOL_CALL.into(),
                        name: "read_record".into(),
                        args: serde_json::json!({"record":"record-7"}),
                        meta: None,
                    })
                })
                .chain(futures::stream::iter([done(StopReason::ToolUse)])),
            );
        }
        // Keep the run alive while the test reads its real row.
        Box::pin(
            futures::stream::once(async move {
                self.ready_to_finish.notify_one();
                self.finish.notified().await;
                Ok(LlmEvent::TextDelta {
                    delta: ANSWER.into(),
                    meta: None,
                })
            })
            .chain(futures::stream::iter([done(StopReason::EndTurn)])),
        )
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct NativeRun {
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    input_id: meerkat_core::InputId,
    completion: meerkat_runtime::completion::CompletionHandle,
}

/// Admit one governed prompt on a native host whose grant owner shares
/// `publication`, served by `provider` and `tools`.
async fn start_native_run(
    provider: Arc<ChurnProvider>,
    publication: LocalAuthorizationPublication,
    tools: Arc<RecordingTools>,
    observer: Option<Arc<RefusalLog>>,
) -> NativeRun {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("native-grants"),
                generation: 1,
            },
            publication,
            Arc::new(HostAuthorizationClock),
        )
        .expect("actual generated grant owner"),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("actual controller lineage");
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("actual operation grant");
    let ingress: Arc<NativeIngressCheck> = Arc::new(|_, _, _, _| Ok(()));
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                retained_resume: None,
                kept_original: None,
                require_scoped_destination: false,
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(RecordOwner),
                configured_mcp_attachment: None,
            })
            .expect("exclusive actual native host setup"),
    );
    let mut factory = AgentFactory::minimal();
    if let Some(observer) = observer {
        factory = factory.with_operation_refusal_observer(observer);
    }
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(provider);
    builder.default_tool_dispatcher = Some(tools);
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
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
    .expect("real native executor and deferred Agent actor");
    let session_id = created.session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("actual live actor");
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("actual immutable selected LlmClientAdapter");
    let credential_owner = machine.generated_auth_lease_handle();
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &credential_owner,
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-loop-credential"),
        tokio::time::Instant::now() + CREDENTIAL_PUBLICATION_WAIT,
    )
    .await
    .expect("actual generated fixture credential before native admission");
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let mut prompt = PromptInput::new("Answer once the owner change completes", None);
    prompt.header.authority_association = Some(association(
        &runtime,
        controller,
        operation,
        pin.selection().clone(),
    ));
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").expect("realm"),
        evidence("fresh-transport-authentication"),
    )
    .expect("current trusted ingress observation")
    .with_controller_client(&input, pin)
    .expect("bind real pin before native acceptance");
    let input = input
        .with_ingress_context(current)
        .expect("exact finalized input binding");
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("real native admission");
    NativeRun {
        machine,
        session_id,
        input_id,
        completion: completion.expect("actual native completion waiter"),
    }
}

/// The run's row-bound audit, as `(operation, observation)` pairs.
async fn row_audit(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &meerkat_core::InputId,
) -> Vec<(OperationId, AuditObservation)> {
    let stored = machine
        .input_state(session_id, input_id)
        .await
        .expect("actual native row query")
        .expect("the running input keeps its row");
    let wire = serde_json::to_value(&stored).expect("row serialization");
    let audit: Vec<StoredAuthorizationAuditObservation> =
        serde_json::from_value(wire["authorization_audit"].clone()).expect("row-bound audit");
    audit
        .into_iter()
        .map(|record| {
            (
                record.observation.operation_id,
                record.observation.observation,
            )
        })
        .collect()
}

/// Drive the run to its answered request, read its audit while the run is
/// still alive, then let it finish.
async fn answered_run(
    provider: &ChurnProvider,
    run: NativeRun,
) -> (Vec<(OperationId, AuditObservation)>, CompletionOutcome) {
    let NativeRun {
        machine,
        session_id,
        input_id,
        completion,
    } = run;
    let mut completion = Box::pin(completion.wait());
    tokio::select! {
        () = provider.ready_to_finish.notified() => {}
        outcome = &mut completion => {
            panic!("the run ended before an answered request: {outcome:?}")
        }
        () = tokio::time::sleep(Duration::from_secs(30)) => {
            panic!("no answered request within 30 s")
        }
    }
    let audit = row_audit(&machine, &session_id, &input_id).await;
    provider.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion)
        .await
        .expect("native run completes")
        .expect("native completion observation");
    provider.join_holders();
    (audit, outcome)
}

fn refusal_notice(messages: &[Message]) -> bool {
    messages.iter().any(|message| {
        matches!(message, Message::SystemNotice(_))
            && serde_json::to_string(message)
                .expect("message serialization")
                .contains("operation_refused")
    })
}

/// Every refusal record names the change it met: still in progress, at the
/// odd publication the held change began.
fn assert_refusals_met(records: &[&AuditObservation], held: OwnerChangeInProgress) {
    for record in records {
        if let AuditObservation::Refused { read, .. } = record {
            let read = read.as_ref().expect("a refusal carries its read");
            assert_eq!(read.basis, AuditRefusalBasis::ChangeInProgress);
            assert_eq!(read.live_sequence, Some(read.publication_sequence));
            assert_eq!(
                OwnerChangeInProgress::new(read.publication_instance, read.publication_sequence),
                held,
                "the refusal read the held change"
            );
        }
    }
}

/// An owner change held across the first model request outlasts the
/// request's immediate rebuild. The request waits for it to complete and
/// rebuilds under the same operation: the run answers with no refusal
/// notice, and the audit holds the refused attempt and one preparation, at
/// the publication the held change completed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_owner_change_is_waited_out_by_the_native_model_request() {
    let publication = LocalAuthorizationPublication::new();
    let provider = Arc::new(ChurnProvider::new(
        Churn::HeldOnce {
            hold: Duration::from_millis(50),
        },
        publication.clone(),
    ));
    let observer = Arc::new(RefusalLog::default());
    let run = start_native_run(
        Arc::clone(&provider),
        publication,
        Arc::new(RecordingTools::default()),
        Some(Arc::clone(&observer)),
    )
    .await;
    let session_id = run.session_id.clone();
    let (audit, outcome) = answered_run(&provider, run).await;

    let requests = provider.requests();
    assert_eq!(requests.len(), 2, "one refused request, then one rebuild");
    let operation = requests[0].0.clone();
    assert!(
        requests.iter().all(|(id, _)| *id == operation),
        "the rebuild keeps its operation"
    );
    // The observer the host installed on its agent factory saw the wait,
    // under the audited operation, and nothing else.
    let observed = observer.observed();
    assert_eq!(observed.len(), 1, "{observed:?}");
    assert_eq!(observed[0].session_id(), &session_id);
    assert_eq!(observed[0].operation_id(), Some(&operation));
    assert_eq!(
        observed[0].surface(),
        &meerkat_core::OperationRefusalSurface::ModelRequest
    );
    assert!(matches!(
        observed[0].disposition(),
        meerkat_core::OperationRefusalDisposition::WaitedForOwnerChange { settled: true, .. }
    ));
    let held = provider
        .first_held
        .lock()
        .expect("held token")
        .expect("a change was in progress at the first request");
    let PolicyPublicationObservation::LocalPublication { instance, sequence } = requests[1]
        .1
        .expect("the answered request observed its publication");
    assert_eq!(
        held,
        OwnerChangeInProgress::new(instance, sequence - 1),
        "the rebuild observed the publication the held change completed"
    );
    let records: Vec<&AuditObservation> = audit
        .iter()
        .filter(|(id, _)| *id == operation)
        .map(|(_, observation)| observation)
        .collect();
    let prepared: Vec<u64> = records
        .iter()
        .filter_map(|observation| match observation {
            AuditObservation::Prepared { policy, .. } => Some(policy.publication_sequence),
            _ => None,
        })
        .collect();
    assert_eq!(prepared, [sequence], "exactly one preparation, at S + 2");
    let refused: Vec<OperationRefusalKind> = records
        .iter()
        .filter_map(|observation| match observation {
            AuditObservation::Refused { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect();
    assert!(
        !refused.is_empty() && refused.len() < requests.len(),
        "at most one refusal per attempt before the change settled: {refused:?}"
    );
    assert!(
        refused
            .iter()
            .all(|reason| *reason == OperationRefusalKind::ReprepareRequired),
        "{refused:?}"
    );
    assert_refusals_met(&records, held);
    assert!(
        !records.iter().any(|observation| matches!(
            observation,
            AuditObservation::AuthorizationUnavailable { .. }
        )),
        "no unavailable record"
    );
    assert_eq!(
        records.len(),
        refused.len() + 3,
        "the refusals, then preparation, entry and outcome"
    );
    assert!(
        !provider
            .requests
            .lock()
            .expect("requests")
            .iter()
            .any(|request| refusal_notice(&request.messages)),
        "no refusal notice reached the model"
    );
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("a waited owner change must not end the run: {outcome:?}")
    };
    assert_eq!(result.text, ANSWER);
    assert!(result.terminal_cause_kind.is_none());
}

/// Changes that keep beginning as each one completes: the first operation
/// waits and rebuilds for free at most three times, takes its own rebuild,
/// and then returns to the controller as without waiting. Its audit holds
/// one refusal per attempt and nothing more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_publication_storm_bounds_native_model_attempts_and_their_audit() {
    let publication = LocalAuthorizationPublication::new();
    let provider = Arc::new(ChurnProvider::new(
        Churn::EveryRequestOfFirstOperation {
            hold: Duration::from_millis(5),
        },
        publication.clone(),
    ));
    let run = start_native_run(
        Arc::clone(&provider),
        publication,
        Arc::new(RecordingTools::default()),
        None,
    )
    .await;
    let (audit, outcome) = answered_run(&provider, run).await;

    let requests = provider.requests();
    let first = requests[0].0.clone();
    let first_attempts = requests.iter().filter(|(id, _)| *id == first).count();
    assert_eq!(
        first_attempts,
        1 + usize::from(OWNER_CHANGE_FREE_REBUILDS) + 1,
        "one attempt, the free rebuilds, then its own rebuild"
    );
    assert_eq!(
        requests.len(),
        first_attempts + 1,
        "then one controller feedback request"
    );
    let first_records: Vec<&AuditObservation> = audit
        .iter()
        .filter(|(id, _)| *id == first)
        .map(|(_, observation)| observation)
        .collect();
    assert_eq!(
        first_records.len(),
        first_attempts,
        "one record per attempt"
    );
    assert!(first_records.iter().all(|observation| matches!(
        observation,
        AuditObservation::Refused {
            reason: OperationRefusalKind::ReprepareRequired,
            ..
        }
    )));
    let notices: Vec<bool> = provider
        .requests
        .lock()
        .expect("requests")
        .iter()
        .map(|request| refusal_notice(&request.messages))
        .collect();
    assert_eq!(
        notices.iter().filter(|notice| **notice).count(),
        1,
        "only the feedback request carries the refusal notice: {notices:?}"
    );
    assert_eq!(notices.last(), Some(&true));
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("the controller feedback request answers: {outcome:?}")
    };
    assert_eq!(result.text, ANSWER);
}

/// An owner change that begins while the model's tool call is on its way to
/// dispatch, held past the dispatch's immediate rebuild. The governed
/// dispatch asks the run's own work authorization, which reports the change
/// through every wrapper; it waits for the change and prepares the same
/// operation again. The body runs once, the model receives its result and no
/// refusal, and the audit holds the refused preparation and one preparation,
/// at the publication the held change completed. A wait that asked a work
/// which does not report the change, as an exact-control work does not,
/// would refuse the tool instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_owner_change_is_waited_out_by_the_native_tool_dispatch() {
    let publication = LocalAuthorizationPublication::new();
    let provider = Arc::new(ChurnProvider::new(
        Churn::HeldOverToolCall {
            hold: Duration::from_millis(500),
        },
        publication.clone(),
    ));
    let tools = Arc::new(RecordingTools::default());
    let observer = Arc::new(RefusalLog::default());
    let run = start_native_run(
        Arc::clone(&provider),
        publication,
        Arc::clone(&tools),
        Some(Arc::clone(&observer)),
    )
    .await;
    let (audit, outcome) = answered_run(&provider, run).await;

    assert_eq!(
        *tools.0.lock().expect("body entries"),
        ["read_record"],
        "the body ran once"
    );
    let feedback = provider
        .requests
        .lock()
        .expect("requests")
        .iter()
        .find_map(|request| tool_feedback(&request.messages, TOOL_CALL))
        .expect("the tool result reached the model");
    assert!(!feedback.is_error, "{}", feedback.text_content());
    assert_eq!(feedback.text_content(), "record-7 value");
    let held = provider
        .first_held
        .lock()
        .expect("held token")
        .expect("a change was in progress at the tool call");
    let answered = provider
        .requests()
        .last()
        .and_then(|(_, observed)| *observed)
        .expect("the answering request observed its publication");
    let PolicyPublicationObservation::LocalPublication { instance, sequence } = answered;
    assert_eq!(
        held,
        OwnerChangeInProgress::new(instance, sequence - 1),
        "the held change completed at the publication the answer observed"
    );
    let tool_operation = audit
        .iter()
        .find_map(|(id, observation)| match observation {
            AuditObservation::Refused { target, .. }
            | AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Tool { call_id, .. }
                    if call_id == TOOL_CALL) =>
            {
                Some(id.clone())
            }
            _ => None,
        })
        .expect("the audit names the tool operation");
    // The factory's observer saw the tool entry's waits, on the call, under
    // its audited operation, and no tool result: the call ran.
    let observed = observer.observed();
    assert!(!observed.is_empty(), "the dispatch's wait was observed");
    for observation in &observed {
        assert_eq!(
            observation.surface(),
            &meerkat_core::OperationRefusalSurface::ToolCall {
                tool_use_id: TOOL_CALL.into()
            }
        );
        assert_eq!(observation.operation_id(), Some(&tool_operation));
        assert!(matches!(
            observation.disposition(),
            meerkat_core::OperationRefusalDisposition::WaitedForOwnerChange { .. }
        ));
    }
    let records: Vec<&AuditObservation> = audit
        .iter()
        .filter(|(id, _)| *id == tool_operation)
        .map(|(_, observation)| observation)
        .collect();
    let refused: Vec<OperationRefusalKind> = records
        .iter()
        .filter_map(|observation| match observation {
            AuditObservation::Refused { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect();
    assert!(
        !refused.is_empty(),
        "the dispatch met the change: {records:?}"
    );
    assert!(
        refused
            .iter()
            .all(|reason| *reason == OperationRefusalKind::ReprepareRequired),
        "{refused:?}"
    );
    assert_refusals_met(&records, held);
    let prepared: Vec<u64> = records
        .iter()
        .filter_map(|observation| match observation {
            AuditObservation::Prepared { policy, .. } => Some(policy.publication_sequence),
            _ => None,
        })
        .collect();
    assert_eq!(prepared, [sequence], "exactly one preparation, at S + 2");
    assert!(
        !records.iter().any(|observation| matches!(
            observation,
            AuditObservation::AuthorizationUnavailable { .. }
        )),
        "no unavailable record"
    );
    assert!(records.iter().any(|observation| matches!(
        observation,
        AuditObservation::Outcome {
            outcome: OperationObservedOutcome::ToolDispatchReturned {
                result_is_error: false,
                ..
            }
        }
    )));
    assert!(
        !provider
            .requests
            .lock()
            .expect("requests")
            .iter()
            .any(|request| refusal_notice(&request.messages)),
        "no refusal notice reached the model"
    );
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("a waited owner change must not end the run: {outcome:?}")
    };
    assert_eq!(result.text, ANSWER);
    assert!(result.terminal_cause_kind.is_none());
}
