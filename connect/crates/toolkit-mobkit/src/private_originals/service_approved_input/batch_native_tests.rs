//! Actual persistent Console A, exact protected set, two stdio attachments and B.
//! Only model output and package readback are mechanical. Native owns queued
//! admission, each operation, cold admitted custody and child process cleanup.
use super::*;
use crate::private_originals::console_ingress::auth_fixture::selected_human;
use crate::private_originals::service_approved_input::admission_tests::batch_recovery_tests::console_task;
use meerkat_authorization_contracts::work_association::{GrantTarget, WorkAuthorityBasis};
use meerkat_core::{ApprovalPrincipalId, ApprovalService};
use tokio::sync::{Notify, watch};

const ALPHA: &str = "batch_alpha";
const BETA: &str = "batch_beta";
const ALPHA_READ: &str = "batch_alpha_read";
const BETA_READ: &str = "batch_beta_read";

struct BatchPackageReadback {
    configurations: Vec<meerkat_core::McpServerConfig>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl ServiceInstallationReadback for BatchPackageReadback {
    async fn readback(
        &self,
        _: &ApprovalRecord,
    ) -> Result<meerkat_core::McpServerConfig, PrivateOriginalError> {
        Err(PrivateOriginalError::Denied)
    }
    async fn readback_source(
        &self,
        source: &ServiceInstallationReadbackSource,
    ) -> Result<meerkat_core::McpServerConfig, PrivateOriginalError> {
        let ServiceInstallationReadbackSource::ReviewedBatchMember(member) = source else {
            return Err(PrivateOriginalError::Denied);
        };
        let selected = self
            .configurations
            .iter()
            .find(|configuration| {
                member.document()["fixture_configuration_sha256"]
                    == digest(&canonical_json(configuration).unwrap())
            })
            .ok_or(PrivateOriginalError::Denied)?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(selected.clone())
    }
}

fn batch_request(
    fixture: &PersistentFixture,
    configuration: Option<&meerkat_core::McpServerConfig>,
) -> meerkat_core::ApprovalRequest {
    use meerkat_core::{
        ApprovalActionKind, ApprovalOwnerRef, ApprovalProposedAction, ApprovalResourceId,
        ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk,
    };
    let body = serde_json::json!({"fixture": "actual-public-service-set",
        "fixture_configuration_sha256": configuration.map(|value| digest(&canonical_json(value).unwrap()))});
    meerkat_core::ApprovalRequest {
        requester: ApprovalPrincipalId::new("fixture-setup-agent").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: fixture.session.clone(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("actual-public-service-set"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Review both public services for this task".into(),
            body: Some(body.clone()),
        },
        risk: ApprovalRisk::Medium,
        request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: Some((std::time::SystemTime::now() + Duration::from_secs(60)).into()),
        metadata: Default::default(),
        request_provenance: None,
    }
}

async fn prepare_batch(
    fixture: &mut PersistentFixture,
    endpoints: &[Endpoint; 2],
) -> (
    PrivateOriginalRef,
    StoredRecord,
    ApprovedTaskLink,
    watch::Sender<bool>,
) {
    let (task, pipe, _logins) = console_task(fixture).await;
    let fresh = PrivateOriginalRef {
        route: task.parent.route.clone(),
        original: task.fresh.clone(),
        event_sha256: String::new(),
    };
    let mut configurations = Vec::new();
    for (index, name) in [ALPHA, BETA].into_iter().enumerate() {
        let mut config = endpoints[index].config(&fixture.scope.realm, true);
        config.name = name.into();
        config.connector = None;
        config.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
        config
            .tool_names
            .insert("read".into(), format!("{name}_read"));
        configurations.push(config);
    }
    let entries = configurations
        .iter()
        .map(|configuration| PublicServiceBatchEntry {
            configuration: configuration.clone(),
            operations: [Id::new("read").unwrap()].into(),
            request: batch_request(fixture, Some(configuration)),
        })
        .collect();
    let prepared = task
        .prepare_public_batch(
            entries,
            Some(60_000),
            batch_request(fixture, None),
            ServiceToolPolicySelection {
                tools: [ALPHA_READ, BETA_READ]
                    .into_iter()
                    .map(
                        |name| crate::installed_host::tool_policy::ServiceToolInvocation {
                            name: name.into(),
                            consequence: meerkat_core::CompiledToolConsequence::R0,
                        },
                    )
                    .collect(),
                catalog: Some(meerkat_core::CompiledToolConsequence::R0),
            },
        )
        .await
        .unwrap();
    let approvals = ApprovalService::new();
    let pending = approvals.request(prepared.request().clone()).unwrap();
    let approved = approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("fixture-protected-set-review").unwrap(),
            None,
            None,
        )
        .unwrap();
    assert!(prepared.approve(approved.clone(), || true).await.unwrap());
    let stored = fixture
        .store
        .get(&fixture.source.key(&fresh).unwrap())
        .await
        .unwrap()
        .unwrap();
    let record = fixture.source.decode_stored(&stored).unwrap();
    let b = PrivateOriginalRef {
        route: record.event.route.clone(),
        original: record.event.original.clone(),
        event_sha256: digest(&canonical_json(&record.event).unwrap()),
    };
    let link = record.approved_task.unwrap();
    assert!(matches!(&link, ApprovedTaskLink::Batch(batch) if batch.members.len() == 2));
    let mut parent_ids = std::collections::BTreeSet::new();
    for grant in link.grants() {
        let access = fixture
            .store
            .get(&fixture.source.access_key(grant).unwrap())
            .await
            .unwrap()
            .unwrap();
        let access = fixture.source.decode_access(&access, grant).unwrap();
        assert_eq!(access.parent.len(), 1);
        assert!(parent_ids.insert(access.parent[0].grant_id.clone()));
        let parent = fixture
            .store
            .get(&toolkit_store::StoreKey {
                realm: fixture.scope.realm.to_string(),
                owner: fixture.scope.owner.clone(),
                id: access.parent[0].grant_id.as_str().to_owned(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parent.data.body["approval"]["approval_id"],
            approved.approval_id.to_string()
        );
    }
    assert_eq!(
        parent_ids.len(),
        3,
        "one real approval yields two independent parents and catalog"
    );
    fixture
        .source
        .bind_service_installation_readback(Arc::new(BatchPackageReadback {
            configurations,
            calls: AtomicUsize::new(0),
        }))
        .unwrap();
    (b, stored, link, pipe)
}

async fn attach_pending(
    fixture: &PersistentFixture,
    endpoints: &[Endpoint; 2],
    b: &PrivateOriginalRef,
    stored: &StoredRecord,
    task: &ApprovedTaskLink,
    history: &dyn RuntimeStore,
) {
    let before = history
        .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    for (index, member) in task.attachments().iter().enumerate() {
        fixture
            .source
            .step_service_input_with_attachment(&fixture.store, stored)
            .await
            .unwrap();
        endpoints[index].wait_for(&endpoints[index].listed).await;
        let held = fixture
            .source
            .held_service_attachment(b, member)
            .unwrap()
            .unwrap();
        let receipt = held
            .receipt(|| panic!("accepted call cannot start twice"))
            .await
            .unwrap()
            .unwrap();
        assert!(receipt.outcome.staged && !receipt.outcome.persisted);
        assert_eq!(
            receipt.attempt.observe("read").await,
            McpAttachmentReadiness::Pending
        );
        let current = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let current = fixture.source.decode_record(&current, b).unwrap();
        assert!(current.final_association.is_none() && current.service_admission.is_none());
        let now = history
            .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&now).unwrap(),
            serde_json::to_value(&before).unwrap(),
            "no B while even one selected attachment remains pending"
        );
        endpoints[index].release();
        assert!(matches!(
            receipt.attempt.wait("read", WAIT).await,
            McpAttachmentReadiness::Usable { .. }
        ));
    }
    for endpoint in endpoints {
        assert_eq!(endpoint.starts(), 1);
        assert!(endpoint.calls().is_empty());
    }
}

async fn revoke_alpha(
    fixture: &PersistentFixture,
    b: &PrivateOriginalRef,
    task: &ApprovedTaskLink,
) {
    let route = fixture.source.route(&b.route).unwrap();
    let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
        authority: route.source_authority.clone(),
        work: b.original.clone(),
    };
    let grant = &task.attachments()[0].grant;
    let lineage = fixture
        .source
        .service_grant_lineage(grant, &original)
        .await
        .unwrap();
    let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
    fixture
        .source
        .owner
        .grants()
        .revoke(&route.grant_caller, lineage.last().unwrap(), &mut custody)
        .unwrap();
}

#[derive(Default)]
struct ScriptState {
    phase: usize,
    last: Option<String>,
    choices: usize,
    results: Vec<ToolResult>,
    contexts: Vec<String>,
}
struct BatchScript {
    armed: AtomicBool,
    revoke: bool,
    state: Mutex<ScriptState>,
    reached: Notify,
    release: Notify,
    paused: AtomicBool,
    finished: AtomicBool,
}
impl BatchScript {
    fn new(revoke: bool) -> Arc<Self> {
        Arc::new(Self {
            armed: AtomicBool::new(false),
            revoke,
            state: Mutex::new(ScriptState::default()),
            reached: Notify::new(),
            release: Notify::new(),
            paused: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        })
    }
    async fn choice(
        &self,
        request: &PreparedLlmRequest,
    ) -> Option<(String, String, serde_json::Value)> {
        let pause = {
            let mut state = self.state.lock().unwrap();
            if let Some(last) = &state.last {
                let feedback = request
                    .request()
                    .messages
                    .iter()
                    .flat_map(|message| match message {
                        Message::ToolResults { results, .. } => results.as_slice(),
                        _ => &[],
                    })
                    .find(|result| &result.tool_use_id == last);
                if let Some(result) = feedback {
                    if result.is_error {
                        let kind = native_public_failure(result);
                        eprintln!("{}", serde_json::json!({
                            "event": "batch_fixture_public_refusal",
                            "phase": state.phase,
                            "choice": state.choices,
                            "public_class": kind,
                        }));
                        if state.phase == 1 && self.revoke {
                            assert_eq!(kind, "authorization_refused");
                            state.results.push(result.clone());
                            state.phase += 1;
                            state.last = None;
                        } else {
                            assert_eq!(
                                kind, "authorization_refused",
                                "bounded mechanical preparation handling requires the exact public refusal"
                            );
                        }
                    } else {
                        assert!(result.settlement_failures.is_empty());
                        if state.phase == 1 {
                            assert!(!self.revoke, "revoked alpha must never return success");
                        }
                        if state.phase == 0 || state.phase == 3 {
                            let catalog: serde_json::Value =
                                serde_json::from_str(&result.text_content()).unwrap();
                            assert_eq!(catalog["catalog_exact"], true);
                            let field = if state.phase == 0 {
                                "accepted_names"
                            } else {
                                "noop_names"
                            };
                            assert_eq!(catalog[field], serde_json::json!([ALPHA_READ, BETA_READ]));
                        }
                        state.results.push(result.clone());
                        state.phase += 1;
                        state.last = None;
                    }
                }
            }
            if state.contexts.is_empty() {
                state.contexts = request
                    .request()
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::User(user) if user.transcript_role.is_injected_context() => {
                            Some(user.text_content())
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    state.contexts.len(),
                    2,
                    "both actual attachment observations precede B"
                );
            }
            state.phase == 1 && !self.paused.swap(true, Ordering::SeqCst)
        };
        if pause {
            self.reached.notify_one();
            self.release.notified().await;
        }
        let mut state = self.state.lock().unwrap();
        if state.phase == 4 {
            self.finished.store(true, Ordering::SeqCst);
            return None;
        }
        assert!(
            state.choices < 16,
            "bounded native preparation retries exhausted"
        );
        state.choices += 1;
        let name = match state.phase {
            0 | 3 => "tool_catalog_load",
            1 => ALPHA_READ,
            2 => BETA_READ,
            _ => unreachable!(),
        };
        assert!(
            request
                .request()
                .tools
                .iter()
                .any(|tool| tool.name.as_str() == name),
            "selected native tool must be offered before the mechanical choice: {name}"
        );
        let args = if state.phase == 0 || state.phase == 3 {
            serde_json::json!({"names":[ALPHA_READ, BETA_READ]})
        } else {
            serde_json::json!({})
        };
        let call = format!("batch-native-{}-{}", state.phase, state.choices);
        state.last = Some(call.clone());
        Some((call, name.into(), args))
    }
}

// ToolResult is the public transcript projection, not the typed terminal cause.
// Core intentionally renders Denied and ReprepareRequired identically here.
// The known alpha revocation plus zero server entry proves nonentry; independent
// beta/catalog success is required separately. Do not infer an internal kind.
fn native_public_failure(result: &ToolResult) -> &'static str {
    for (kind, error) in [
        (
            "authorization_refused",
            meerkat_core::ToolError::AuthorizationRefused {
                refusal: meerkat_core::OperationRefused::new(
                    meerkat_core::OperationRefusalKind::Denied,
                ),
            },
        ),
        (
            "authorization_unavailable",
            meerkat_core::ToolError::OperationAuthorizationUnavailable,
        ),
    ] {
        let expected =
            meerkat_core::ops::terminal_tool_outcome_for_error(result.tool_use_id.clone(), error)
                .result;
        if serde_json::to_value(result).unwrap() == serde_json::to_value(expected).unwrap() {
            return kind;
        }
    }
    panic!("expected exact public native authorization failure");
}

struct BatchProvider {
    inner: Arc<dyn LlmClient>,
    script: Arc<BatchScript>,
}
#[async_trait::async_trait]
impl LlmClient for BatchProvider {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }
    fn plain_model_route(
        &self,
        model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(model)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        self.inner.controller_model_selection()
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let stream = self.inner.stream_prepared(request);
        if !self.script.armed.load(Ordering::SeqCst) {
            return stream;
        }
        Box::pin(futures::stream::unfold(
            (stream, false),
            move |(mut stream, mut tool)| async move {
                let event = stream.next().await?;
                let event = match event {
                    Ok(LlmEvent::TextDelta { meta, .. }) => match self.script.choice(request).await
                    {
                        Some((id, name, args)) => {
                            tool = true;
                            Ok(LlmEvent::ToolCallComplete {
                                id,
                                name,
                                args,
                                meta: None,
                            })
                        }
                        None => Ok(LlmEvent::TextDelta {
                            delta: "Both selected targets were checked through native authority"
                                .into(),
                            meta,
                        }),
                    },
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success { .. },
                    }) if tool => Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                    }),
                    other => other,
                };
                Some((event, (stream, tool)))
            },
        ))
    }
    fn provider(&self) -> Provider {
        self.inner.provider()
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}

async fn assert_completed(
    fixture: &PersistentFixture,
    history: &dyn RuntimeStore,
    b: &PrivateOriginalRef,
) -> SourceRecord {
    tokio::time::timeout(WAIT, async {
        loop {
            let stored = fixture
                .store
                .get(&fixture.source.key(b).unwrap())
                .await
                .unwrap()
                .unwrap();
            let record = fixture.source.decode_record(&stored, b).unwrap();
            let admission = record.service_admission.as_ref().unwrap();
            let rows = history
                .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
                .await
                .unwrap();
            assert_eq!(rows.len(), 2);
            let row = rows
                .iter()
                .find(|row| row.state.input_id.to_string() == admission.input_id)
                .unwrap();
            if let Some(run) = &row.seed.last_run_id {
                if row.seed.phase == InputLifecycleState::Consumed
                    && fixture
                        .machine
                        .run_turn_terminal(&fixture.session, run)
                        .await
                        == Some(TurnTerminalOutcome::Completed)
                {
                    let receipt = fixture
                        .machine
                        .input_terminal_receipt(
                            &fixture.session,
                            InteractionSelector::InputId(row.state.input_id.clone()),
                        )
                        .await
                        .unwrap()
                        .unwrap();
                    let InputTerminalReceiptRead::Finalized(receipt) = receipt.report else {
                        continue;
                    };
                    let CompletionOutcome::Completed(result) = receipt.outcome() else {
                        panic!("B must complete");
                    };
                    assert_eq!(
                        result.text,
                        "Both selected targets were checked through native authority"
                    );
                    return record;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

/// Exercise the exact source callback for an admitted contributor using an
/// actual native FirstAuthentication observation. The different occurrence is
/// lookup context only: this does not admit or manufacture a new Original C.
fn assert_admitted_contributor(
    fixture: &PersistentFixture,
    association: &InputAuthorityAssociation,
    ingress: &meerkat_runtime::input_authority::NativeIngressContext,
) {
    assert!(matches!(
        ingress.replay_lookup(),
        Some(meerkat_runtime::input_authority::NativeReplayLookup::FirstAuthentication)
    ));
    let claims = association.candidate();
    let mut incoming = claims.ingress_namespace.clone();
    incoming.occurrence_scope = EvidenceId::new("another-incoming-occurrence").unwrap();
    assert_ne!(
        incoming.occurrence_scope,
        claims.ingress_namespace.occurrence_scope
    );
    let selected = fixture
        .source
        .owner
        .authenticated_original(ingress, &incoming, &claims.original_work)
        .unwrap()
        .unwrap();
    assert_eq!(selected.authority_basis, claims.authority_basis);
    assert_eq!(
        selected.occurrence_scope,
        claims.ingress_namespace.occurrence_scope
    );
}

async fn actual_batch_case(revoke_and_restore: bool) {
    let script = BatchScript::new(revoke_and_restore);
    let selected = script.clone();
    let ingress_observations = Arc::new(Mutex::new(Vec::new()));
    let (mut fixture, history) = PersistentFixture::for_console_service_batch_with_ingress(
        selected_human(),
        move |inner| {
            Arc::new(BatchProvider {
                inner,
                script: selected,
            })
        },
        ingress_observations.clone(),
    )
    .await;
    let endpoints = [Endpoint::new(), Endpoint::new()];
    let outcome = AssertUnwindSafe(tokio::time::timeout(WAIT * 6, async {
        let (b, stored, task, _pipe) = prepare_batch(&mut fixture, &endpoints).await;
        attach_pending(&fixture, &endpoints, &b, &stored, &task, history.as_ref()).await;
        script.armed.store(true, Ordering::SeqCst);
        fixture
            .source
            .step_service_input_with_attachment(&fixture.store, &stored)
            .await
            .unwrap();
        tokio::time::timeout(WAIT, script.reached.notified())
            .await
            .unwrap();
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = fixture.source.decode_record(&saved, &b).unwrap();
        let association = record.final_association.as_ref().unwrap();
        let WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(base),
            targets,
        } = &association.candidate().authority_basis
        else {
            panic!("independent batch lineages required");
        };
        assert_eq!(targets.len(), 2);
        assert!(
            association
                .candidate()
                .authority_basis
                .lineage_for(Some(&GrantTarget::McpServer {
                    server: "unselected".into()
                }))
                .is_none()
        );
        assert!(
            targets
                .iter()
                .all(|target| target.lineage.last() != base.last())
        );
        assert!(record.service_admission.is_some());
        if revoke_and_restore {
            revoke_alpha(&fixture, &b, &task).await;
            let ingress = ingress_observations
                .lock()
                .unwrap()
                .iter()
                .find(|(selected, _)| selected == association)
                .unwrap()
                .1
                .clone();
            assert_admitted_contributor(&fixture, association, &ingress);
        }
        script.release.notify_one();
        let completed = assert_completed(&fixture, history.as_ref(), &b).await;
        assert!(script.finished.load(Ordering::SeqCst));
        assert_eq!(
            endpoints[0].calls().len(),
            if revoke_and_restore { 0 } else { 1 }
        );
        assert_eq!(
            endpoints[1].calls().len(),
            1,
            "independent beta target survives alpha-only revoke"
        );
        let results = script.state.lock().unwrap().results.clone();
        assert_eq!(results.len(), 4);
        assert_eq!(results[1].is_error, revoke_and_restore);
        assert!(
            !results[2].is_error && !results[3].is_error,
            "beta and catalog retain their own native authority"
        );
        if revoke_and_restore {
            // Native historical observation acquires the turn boundary. The
            // actual X/Y/catalog run must settle before forgetting its source
            // custody; replacing it under the paused live turn would deadlock
            // the observer against the provider gate. This covers settled
            // historical reconstruction, not a process restart or a new run.
            fixture
                .source
                .retained
                .lock()
                .unwrap()
                .retain(|held| held.reference != b);
            fixture
                .source
                .owner
                .forget_task_custody_for_test(association);
            let restored = fixture
                .source
                .restore_service_admission(&b)
                .await
                .unwrap()
                .unwrap();
            assert!(Some(restored.admission) == completed.service_admission);
            let ingress = ingress_observations
                .lock()
                .unwrap()
                .iter()
                .find(|(selected, _)| selected == association)
                .unwrap()
                .1
                .clone();
            assert_admitted_contributor(&fixture, association, &ingress);
            assert_eq!(
                fixture.store.get(&stored.key).await.unwrap().unwrap(),
                saved,
                "cold custody observation changes no retained input or source bytes"
            );
        }
        let before = history
            .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
            .await
            .unwrap();
        fixture
            .source
            .step_service_input_with_attachment(&fixture.store, &saved)
            .await
            .unwrap();
        let after = history
            .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&after).unwrap(),
            serde_json::to_value(&before).unwrap(),
            "exact replay does not readmit B"
        );
        // Actual admitted B cancellation must still settle the existing owner;
        // its three published grants are not pre-admission compensation work.
        let grants_before =
            std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap();
        fixture
            .source
            .record_task_group_cancelled(&task.parent)
            .await
            .unwrap();
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        fixture
            .source
            .step_service_input_with_attachment(&fixture.store, &saved)
            .await
            .unwrap();
        let closed = fixture
            .source
            .decode_record(&fixture.store.get(&stored.key).await.unwrap().unwrap(), &b)
            .unwrap();
        assert!(closed.operation_cancelled);
        assert!(closed.service_admission == completed.service_admission);
        assert_eq!(
            std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap(),
            grants_before
        );
    }))
    .catch_unwind()
    .await;
    script.release.notify_one();
    for endpoint in &endpoints {
        endpoint.release();
    }
    fixture.source.join_service_attachments().await;
    ingress_observations.lock().unwrap().clear();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("actual batch control timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_batch_waits_for_both_attachments_and_admits_one_original() {
    actual_batch_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_batch_target_independence_and_settled_cold_restore_after_alpha_revoke() {
    actual_batch_case(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_batch_rechecks_every_member_at_native_first_authentication() {
    let (mut fixture, history) =
        PersistentFixture::for_console_service_batch(selected_human(), |inner| inner).await;
    let endpoints = [Endpoint::new(), Endpoint::new()];
    let mut release = None;
    let outcome = AssertUnwindSafe(tokio::time::timeout(WAIT * 6, async {
        let (b, stored, task, _pipe) = prepare_batch(&mut fixture, &endpoints).await;
        attach_pending(&fixture, &endpoints, &b, &stored, &task, history.as_ref()).await;
        let runtime = fixture.source.bound_identity_runtime().unwrap();
        let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.member.clone();
        let identity = runtime.owned_identity_for_member_alias(&member).await.unwrap();
        let (entered, sender) = meerkat_mob::MobHandle::arm_member_turn_admission_test_gate(meerkat_mob::AgentIdentity::from(identity.as_str()),
            meerkat_mob::MemberTurnAdmissionTestStage::BeforeRuntimeAdmission);
        release = Some(sender);
        let mut submission = Box::pin(fixture.source.step_service_input_with_attachment(&fixture.store, &stored));
        tokio::select! {
            value = &mut submission => panic!("B must reach the actual native actor gate: {value:?}"),
            value = entered => value.unwrap(),
        }
        // Actual SubmitWork has been queued, the host fence already returned,
        // and no native runtime admission has occurred at this existing gate.
        let pending = fixture.source.decode_record(&fixture.store.get(&stored.key).await.unwrap().unwrap(), &b).unwrap();
        assert!(pending.final_association.is_some() && pending.bound_service_input.is_some());
        assert!(pending.service_admission.is_none());
        let before = history.load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session)).await.unwrap();
        assert_eq!(before.len(), 1);
        let model_requests = fixture.model_requests();
        revoke_alpha(&fixture, &b, &task).await;
        release.take().unwrap().send(()).unwrap();
        assert!(tokio::time::timeout(WAIT, &mut submission).await.unwrap().is_err());
        drop(submission);
        let saved = fixture.source.decode_record(&fixture.store.get(&stored.key).await.unwrap().unwrap(), &b).unwrap();
        assert!(saved.service_admission.is_none(), "host precheck cannot admit stale set while queued");
        assert_eq!(fixture.model_requests(), model_requests);
        let after = history.load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session)).await.unwrap();
        assert_eq!(serde_json::to_value(&after).unwrap(), serde_json::to_value(&before).unwrap());
        assert!(endpoints.iter().all(|endpoint| endpoint.calls().is_empty()));
    })).catch_unwind().await;
    drop(release);
    for endpoint in &endpoints {
        endpoint.release();
    }
    fixture.source.join_service_attachments().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("queued batch control timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[path = "batch_attachment_failure_tests.rs"]
mod attachment_failure;
