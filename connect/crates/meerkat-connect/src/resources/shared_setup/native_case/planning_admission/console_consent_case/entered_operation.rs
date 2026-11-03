//! Actual registered Find entry followed by the production graph fence.
//! These cases do not claim an all-role helper batch or a fault between child
//! grant appends. The existing Console negatives and custody controls remain.
use super::*;
use meerkat_core::approval::review::{
    OperationReviewObserver, ReviewAttemptStatus, ReviewObservation, ReviewRetirementReason,
};
use meerkat_core::authorization::{AuthorizationOperation, PreparedAuthorizationBinding};
use meerkat_workgraph::{WorkAdmission, WorkItemAdmissionState};
use toolkit_mobkit::private_originals::PrivateOriginalError;

#[test]
fn registered_find_declares_consuming_entry_only_for_its_exact_leaf() {
    use meerkat_core::approval::review::ReviewEntrySupport;
    let tools = InstalledAcquisitionTools::new(Duration::from_secs(10)).unwrap();
    let leaf = tools.acquisition_tool();
    let registered = meerkat_core::RegisteredToolBundleDispatcher::new(
        toolkit_mobkit::FIND_AND_INSTALL_BUNDLE,
        leaf.clone(),
    );
    for dispatcher in [&*leaf, &registered as &dyn AgentToolDispatcher] {
        assert_eq!(
            dispatcher.review_entry_support(toolkit_mobkit::FIND_AND_INSTALL),
            ReviewEntrySupport::ConsumesAtEntry
        );
        for name in [
            "",
            "find_and_install_extra",
            "prefix_find_and_install",
            "tool_catalog_search",
        ] {
            assert_eq!(
                dispatcher.review_entry_support(name),
                ReviewEntrySupport::Unsupported
            );
        }
    }
}

#[derive(Default)]
pub(super) struct ExactFindReview {
    calls: Mutex<BTreeMap<String, Vec<(Value, PreparedAuthorizationBinding)>>>,
    observations: Mutex<Vec<ReviewObservation>>,
    used: tokio::sync::Notify,
    dispatch_order: DispatchOrder,
}

struct DispatchOrder {
    ready: [tokio::sync::Semaphore; 2],
    release: [tokio::sync::Semaphore; 2],
}
impl Default for DispatchOrder {
    fn default() -> Self {
        Self {
            ready: std::array::from_fn(|_| tokio::sync::Semaphore::new(0)),
            release: std::array::from_fn(|_| tokio::sync::Semaphore::new(0)),
        }
    }
}

// Release held provider responses before native cleanup on every fixture exit.
// These permits schedule model output only; they convey no native authority.
struct ReleaseDispatches(Arc<ExactFindReview>);
impl Drop for ReleaseDispatches {
    fn drop(&mut self) {
        for release in &self.0.dispatch_order.release {
            release.add_permits(1);
        }
    }
}
impl OperationReviewObserver for ExactFindReview {
    fn observe(&self, observation: ReviewObservation) {
        let used = observation.status == ReviewAttemptStatus::Used;
        self.observations.lock().unwrap().push(observation);
        if used {
            self.used.notify_one();
        }
    }
}
impl ExactFindReview {
    pub(super) async fn response(
        &self,
        script: &PlanningScript,
        request: &LlmRequest,
    ) -> Vec<LlmEvent> {
        let events = script.response(request);
        for event in &events {
            if let LlmEvent::ToolCallComplete { id, name, .. } = event {
                assert_eq!(name, toolkit_mobkit::FIND_AND_INSTALL);
                let index = match id.as_str() {
                    "a-planning-find" => 0,
                    "b-planning-find" => 1,
                    _ => panic!("only the two exact R2 Find calls are scheduled"),
                };
                self.dispatch_order.ready[index].add_permits(1);
                self.dispatch_order.release[index]
                    .acquire()
                    .await
                    .unwrap()
                    .forget();
            }
        }
        // ToolCallComplete has not reached Core while held. No native review
        // candidate or entered witness is cached across this scheduling wait.
        events
    }

    async fn await_used(&self, call: &str) {
        loop {
            let notified = self.used.notified();
            if self.observations.lock().unwrap().iter().any(|row| {
                row.tool_call_id.as_ref() == call && row.status == ReviewAttemptStatus::Used
            }) {
                break;
            }
            notified.await;
        }
    }

    pub(super) async fn before_waiter_turns(
        &self,
        planning: &PlanningCase,
        case: &Case,
        observations: &[Value],
    ) {
        // The caller has already received both real FreshAcceptancePromoted
        // launch replies. Hold B before dispatch while A's entered owners do
        // their actual work. A stays live, so B still joins the same G.
        tokio::time::timeout(BOUND, async {
            for ready in &self.dispatch_order.ready { ready.acquire().await.unwrap().forget(); }
            assert!(self.calls.lock().unwrap().is_empty());
            assert!(self.observations.lock().unwrap().is_empty());
            let a = observations.iter().find(|row| row["original"]["route"] == "original-a").unwrap();
            assert_eq!(a["session"], case.audit_sessions["original-a"].to_string());
            self.dispatch_order.release[0].add_permits(1);
            await_turn(case, "original-a", a["ticket"].as_str().unwrap()).await;
            planning.await_automatic_context_deliveries(case, 1).await;
            self.await_used("a-planning-find").await;

            let (workflow, effects, acquisitions) = workflow_evidence(case);
            let waiters = workflow["waiters"].as_object().unwrap();
            assert_eq!(waiters.len(), 1);
            assert_eq!(acquisitions.len(), 1);
            let waiter = waiters.values().next().unwrap();
            let original = serde_json::from_value(a["original"].clone()).unwrap();
            assert_eq!(waiter["identity"]["source"], json!(planning.source.source_record_key(&original).await.unwrap()));
            assert_eq!(waiter["identity"]["work"], a["work"]);
            assert_eq!(waiter["departed"], false);
            assert_eq!(acquisitions[0]["key"], waiter["identity"]["acquisition"]);
            let deliveries = effects.iter().filter(|effect| effect.intent.kind == "setup_workflow_delivery").collect::<Vec<_>>();
            assert_eq!(deliveries.len(), 1);
            assert_eq!(deliveries[0].intent.request.body["payload"]["identity"], waiter["identity"]);
            let launches = effects.iter().filter(|effect| effect.intent.kind == "setup_workflow_launch").collect::<Vec<_>>();
            assert_eq!(launches.len(), 1);
            assert_eq!(launches[0].status, toolkit_store::EffectStatus::Succeeded);
            assert_eq!(launches[0].attempt, 1);
            let Some(toolkit_store::EffectOutcome::Succeeded { receipt }) = &launches[0].outcome else {
                panic!("the actual G launch must have its successful receipt")
            };
            assert_eq!(receipt.schema, "toolkit.setup-workflow-launch");
            assert_eq!(receipt.version, 1);
            assert_eq!(receipt.body["state"], "native");
            assert_eq!(receipt.body.as_object().unwrap().len(), 2);
            let observed: PrivateOriginalSubmission =
                serde_json::from_value(receipt.body["observation"].clone()).unwrap();
            let PrivateOriginalSubmission::Admitted { session_id, input_id,
                source_settlement: toolkit_mobkit::private_originals::PrivateSourceSettlement::FreshAcceptancePromoted, .. }
                = observed else { panic!("the real G launch source must be promoted") };
            assert_eq!(session_id, planning.member.session().unwrap().to_string());
            let inputs = input_ids(case, "planning-lead").await;
            assert_eq!(inputs.len(), 2, "one G launch and A's exact context delivery");
            assert!(inputs.contains(&input_id));
            use meerkat_runtime::SessionServiceRuntimeExt;
            let service = case.native.as_ref().unwrap().mob_runtime().session_service().unwrap();
            let machine = service.acquire_runtime_adapter(None).unwrap().unwrap();
            for input in inputs {
                let input = serde_json::from_value(json!(input)).unwrap();
                loop {
                    match machine.input_terminal_completion(&planning.member.session().unwrap(), &input).await.unwrap() {
                        Some(meerkat_runtime::completion::CompletionOutcome::Completed(_)) => break,
                        Some(_) => panic!("the exact G input must have a finalized successful completion"),
                        None => tokio::time::sleep(Duration::from_millis(10)).await,
                    }
                }
            }
            assert_eq!(self.calls.lock().unwrap().len(), 1);
            assert!(!self.calls.lock().unwrap().contains_key("b-planning-find"));
            // This excludes A's observed pending work, not every background
            // reader. Exact held-source recovery remains a no-op replay;
            // expiry, cancellation or changed ownership still fail closed.
            self.dispatch_order.release[1].add_permits(1);
        }).await.expect("actual A owners and G receipts must settle before B stages its independent review");
    }

    async fn assert_final_native_attempts(&self) {
        tokio::time::timeout(BOUND, async {
            loop {
                if self
                    .observations
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|row| row.status == ReviewAttemptStatus::Used)
                    .count()
                    >= 2
                {
                    break;
                }
                self.used.notified().await;
            }
        })
        .await
        .expect("the two actual entered Find owners must release native review custody");
        let callbacks = self
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(call, candidates)| (call.clone(), candidates.len()))
            .collect::<BTreeMap<_, _>>();
        let rows = self.observations.lock().unwrap();
        assert!(rows.iter().all(|row| matches!(
            row.tool_call_id.as_ref(),
            "a-planning-find" | "b-planning-find"
        )));
        use ReviewAttemptStatus::{Allowed, Pending, Retired, Used};
        for call in ["a-planning-find", "b-planning-find"] {
            let observed: Vec<_> = rows
                .iter()
                .filter(|row| row.tool_call_id.as_ref() == call)
                .collect();
            let mut attempts: Vec<Vec<&ReviewObservation>> = Vec::new();
            for row in observed {
                if attempts
                    .last()
                    .is_none_or(|attempt| attempt[0].attempt != row.attempt)
                {
                    assert!(
                        !attempts
                            .iter()
                            .any(|attempt| attempt[0].attempt == row.attempt),
                        "a retired or used attempt must not reopen"
                    );
                    attempts.push(vec![]);
                }
                attempts.last_mut().unwrap().push(row);
            }
            assert!(
                (1..=2).contains(&attempts.len()),
                "Core permits one pre-entry supersession only"
            );
            assert_eq!(
                callbacks[call],
                attempts.len(),
                "one reviewer call per actual native attempt"
            );
            let final_attempt = attempts.pop().unwrap();
            assert_eq!(
                final_attempt
                    .iter()
                    .map(|row| (row.status, row.retirement))
                    .collect::<Vec<_>>(),
                vec![(Pending, None), (Allowed, None), (Used, None)],
                "the final actual attempt spends once and is Used only after its owners release"
            );
            if let Some(predecessor) = attempts.pop() {
                let statuses = predecessor
                    .iter()
                    .map(|row| (row.status, row.retirement))
                    .collect::<Vec<_>>();
                assert!(
                    statuses
                        == vec![
                            (Pending, None),
                            (Retired, Some(ReviewRetirementReason::ContextChanged))
                        ]
                        || statuses
                            == vec![
                                (Pending, None),
                                (Allowed, None),
                                (Retired, Some(ReviewRetirementReason::ContextChanged))
                            ],
                    "only a real pre-entry ContextChanged attempt may precede the entered attempt"
                );
            }
        }
    }
}
#[async_trait]
impl OperationReviewer for ExactFindReview {
    async fn review(
        &self,
        candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        let tag = match candidate.tool_call_id() {
            "a-planning-find" => "a",
            "b-planning-find" => "b",
            _ => return Err(ReviewerFailure::Unavailable),
        };
        if candidate.tool_name() != toolkit_mobkit::FIND_AND_INSTALL {
            return Err(ReviewerFailure::Unavailable);
        }
        let arguments: Value = serde_json::from_str(candidate.arguments().get())
            .map_err(|_| ReviewerFailure::Unavailable)?;
        if arguments
            != json!({"request":format!("sdk-planning-{tag}: prepare this independently authenticated request")})
        {
            return Err(ReviewerFailure::Unavailable);
        }
        let AuthorizationOperation::Tool(tool) = &candidate.binding().facts().operation else {
            return Err(ReviewerFailure::Unavailable);
        };
        assert_eq!(tool.name, candidate.tool_name());
        assert_eq!(tool.arguments.get(), candidate.arguments().get());
        self.calls
            .lock()
            .unwrap()
            .entry(candidate.tool_call_id().into())
            .or_default()
            .push((arguments, candidate.binding().clone()));
        // Callback count is not spend count. Native may supersede a review
        // before any dispatcher code runs. The real observer below checks
        // exact attempt identity, ContextChanged retirement and final Used.
        // This is an explicitly selected test reviewer, not a grant issuer.
        // Real installed policy, source currentness and native entry still run.
        Ok(ReviewVerdict::Allow)
    }
}

fn r2(root: PathBuf) -> (Arc<ConsentControl>, Arc<ExactFindReview>) {
    let reviewer = Arc::new(ExactFindReview::default());
    let mut control = ConsentControl::new(root);
    Arc::get_mut(&mut control).unwrap().r2 = Some(reviewer.clone());
    (control, reviewer)
}

async fn graph_row(
    planning: &PlanningCase,
    transfer: &context_transfer_tests::Transfer,
) -> (meerkat_workgraph::WorkItem, WorkItemAdmissionState) {
    let call = match transfer.0.route.as_str() {
        "original-a" => "a-planning-find",
        "original-b" => "b-planning-find",
        _ => panic!("only the selected actual waiter"),
    };
    let result = planning.script.results.lock().unwrap()[call].clone();
    assert_eq!(result["worker"]["context_transfer"]["state"], "committed");
    let expected: WorkAdmission =
        serde_json::from_value(result["worker"]["context_transfer"]["admission"].clone()).unwrap();
    let row = transfer
        .2
        .store()
        .observe_item_admission(
            transfer.2.default_realm_id(),
            transfer.2.default_namespace(),
            &expected.key,
        )
        .await
        .unwrap()
        .unwrap();
    let expected_state: WorkItemAdmissionState = serde_json::from_value(json!({
        "lifecycle_phase":"admitted", "admission_key":expected.key,
        "request_digest":expected.request_digest,
    }))
    .unwrap();
    assert_eq!(row.1, expected_state);
    let envelope: Value = serde_json::from_str(row.0.description.as_deref().unwrap()).unwrap();
    assert_eq!(envelope["purpose"], "toolkit-workflow-inline-context-v1");
    assert_eq!(envelope["recipient"]["original"], json!(transfer.0));
    assert_eq!(
        envelope["recipient"]["planning_source"],
        json!(planning.lead.original)
    );
    let parsed: meerkat_toolkit::request::FindAndInstallRequest =
        serde_json::from_value(planning.script.calls.lock().unwrap()[call].clone()).unwrap();
    assert_eq!(envelope["request"], json!(parsed));
    row
}

#[test]
fn registered_r2_find_enters_once_per_call_and_commits_real_context_graphs() {
    let (pipes, parent) = SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let mut case = Case::new(parent.enter());
            let (control, reviewer) = r2(case.directory.path().canonicalize().unwrap());
            let release_dispatches = ReleaseDispatches(reviewer.clone());
            let body = std::panic::AssertUnwindSafe(async {
                let planning =
                    construction::construct_with_consent(&mut case, pipes, control.clone()).await;
                case.host
                    .as_ref()
                    .unwrap()
                    .publish_runtime_bootstrap_completed(case.native.as_ref().unwrap())
                    .unwrap();
                let launch = case.launch.as_ref().unwrap().clone();
                planning
                    .script
                    .allow_context_delivery
                    .store(true, Ordering::Release);
                launch.enable_context_transfers(planning.source.clone());
                // This existing helper asserts the real inputs, distinct A/B work,
                // one G admission, both actual delivery receipts and no replay.
                let (a, b) = planning.run_open(&mut case).await;
                let transfers = launch.context_transfers();
                assert_eq!(transfers.len(), 2);
                let mut before = Vec::new();
                for transfer in &transfers {
                    before.push(graph_row(&planning, transfer).await);
                }
                assert_ne!(before[0].0.id, before[1].0.id);
                {
                    let calls = reviewer.calls.lock().unwrap();
                    assert_eq!(calls.len(), 2);
                    for call in ["a-planning-find", "b-planning-find"] {
                        assert!(!calls[call].is_empty());
                        for (arguments, _) in &calls[call] {
                            assert_eq!(arguments, &planning.script.calls.lock().unwrap()[call]);
                        }
                    }
                    for (_, a) in &calls["a-planning-find"] {
                        for (_, b) in &calls["b-planning-find"] {
                            assert!(
                                !a.same_operation(b),
                                "independent inputs never share one operation binding"
                            );
                        }
                    }
                }
                assert!(
                    control
                        .issuer()
                        .approvals()
                        .list(ApprovalListFilter::default())
                        .unwrap()
                        .is_empty(),
                    "R2 allow does not fabricate a human consent record"
                );
                let inputs = input_ids(&case, "planning-lead").await;
                for original in [a, b] {
                    let cancelled = case
                        .client
                        .lock()
                        .await
                        .call(json!({"kind":"cancel_original_operation", "original":original}))
                        .await
                        .unwrap();
                    assert_eq!(cancelled["kind"], "original_operation_cancelled");
                }
                for (transfer, prior) in transfers.iter().zip(before) {
                    assert!(matches!(
                        transfer.1.observe().await,
                        Err(PrivateOriginalError::Denied)
                    ));
                    assert_eq!(
                        graph_row(&planning, transfer).await,
                        prior,
                        "later refusal cannot erase an independently committed graph effect"
                    );
                }
                assert_eq!(input_ids(&case, "planning-lead").await, inputs);
                assert_eq!(reviewer.calls.lock().unwrap().len(), 2);
                planning.cancel_parent(&mut case).await;
            })
            .catch_unwind()
            .await;
            drop(release_dispatches);
            if body.is_err() {
                control.release.add_permits(8);
                case.retain_failed_audits().await;
            }
            let cleanup = construction::cleanup_completed_runtime(&mut case).await;
            if let Err(panic) = body {
                eprintln!("entered R2 fixture cleanup error count: {}", cleanup.len());
                std::panic::resume_unwind(panic);
            }
            assert!(
                cleanup.is_empty(),
                "actual SDK/native owners must join: {cleanup:?}"
            );
            reviewer.assert_final_native_attempts().await;
        });
}

#[test]
fn registered_r3_find_spends_exact_consent_and_preserves_its_committed_graph_after_close() {
    let (pipes, parent) = SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let mut case = Case::new(parent.enter());
            let control = ConsentControl::new(case.directory.path().canonicalize().unwrap());
            let body = std::panic::AssertUnwindSafe(async {
                let planning =
                    construction::construct_with_consent(&mut case, pipes, control.clone()).await;
                case.host
                    .as_ref()
                    .unwrap()
                    .publish_runtime_bootstrap_completed(case.native.as_ref().unwrap())
                    .unwrap();
                let launch = case.launch.as_ref().unwrap().clone();
                planning
                    .script
                    .allow_context_delivery
                    .store(true, Ordering::Release);
                launch.enable_context_transfers(planning.source.clone());
                // Preserve the complete original authenticated R3 exercise: real
                // refusal, exact approval/input/call, decision-only non-entry,
                // consumed_at and one retained-attempt native result notice.
                exercise(&mut case, &planning, &control, Change::Approve).await;
                let approvals = control
                    .issuer()
                    .approvals()
                    .list(ApprovalListFilter::default())
                    .unwrap();
                assert_eq!(approvals.len(), 1);
                assert_eq!(approvals[0].status, ApprovalStatus::Approved);
                assert!(approvals[0].consent.as_ref().unwrap().consumed_at.is_some());
                let transfers = launch.context_transfers();
                assert_eq!(transfers.len(), 1);
                let committed = graph_row(&planning, &transfers[0]).await;
                assert_eq!(input_count(&case, "original-a").await, 1);
                assert_eq!(input_count(&case, "original-b").await, 0);
                // exercise closed the actual P source. Observing a closed owner
                // is a refusal, not rollback of the already admitted graph row.
                assert!(matches!(
                    transfers[0].1.observe().await,
                    Err(PrivateOriginalError::Denied)
                ));
                assert_eq!(graph_row(&planning, &transfers[0]).await, committed);
                assert_eq!(
                    control
                        .issuer()
                        .approvals()
                        .list(ApprovalListFilter::default())
                        .unwrap(),
                    approvals
                );
                assert_eq!(planning.script.calls.lock().unwrap().len(), 1);
                assert_eq!(planning.script.results.lock().unwrap().len(), 1);
            })
            .catch_unwind()
            .await;
            if body.is_err() {
                control.release.add_permits(8);
                case.retain_failed_audits().await;
            }
            control.release.add_permits(8);
            let cleanup = construction::cleanup_completed_runtime(&mut case).await;
            if let Err(panic) = body {
                eprintln!("entered R3 fixture cleanup error count: {}", cleanup.len());
                std::panic::resume_unwind(panic);
            }
            assert!(
                cleanup.is_empty(),
                "actual SDK/native owners must join: {cleanup:?}"
            );
        });
}
