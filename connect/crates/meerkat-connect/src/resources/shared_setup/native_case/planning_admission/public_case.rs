//! Opt-in full-resource SDK consumer acceptance. Model decisions are controlled;
//! installation, protected decisions, native inputs and one public MCP read are
//! real. The executor must package the exact current Python resource closure.
use super::*;
#[path = "public_artifacts.rs"]
mod public_artifacts;
#[path = "public_construction.rs"]
mod public_construction;
#[path = "public_model.rs"]
mod public_model;
use public_model::{OPERATION, PublicScript};
const PUBLIC_BOUND: Duration = Duration::from_secs(240);
struct PublicPlanning {
    source: Arc<PrivateOriginals>,
    parent: PrivateOriginalSubmit,
    lead: PrivateOriginalSubmit,
    terms: SetupWorkflowTerms,
    lineage: Vec<GrantLineageRef>,
    member: Arc<InstalledWorkflowMember>,
}
struct Constructed {
    host: Option<Arc<ToolkitGatewayHost>>,
    private: Option<PrivateHostConnection>,
    launch: Arc<SdkLaunchOwner>,
    remote: Option<toolkit_mobkit::private_host::RemoteControllerReviews>,
    planning: Option<PublicPlanning>,
}
async fn exercise(case: &mut Case, constructed: &mut Constructed, script: &Arc<PublicScript>) {
    let planning = constructed
        .planning
        .as_ref()
        .expect("actual installed planning selection");
    assert_eq!(
        planning.member.session().unwrap(),
        case.audit_sessions["planning-lead"]
    );
    let launch = constructed.launch.clone();
    let reply = case
        .client
        .lock()
        .await
        .call(json!({"kind":"submit_authenticated_original","submission":planning.parent}))
        .await
        .unwrap();
    let observation: PrivateOriginalSubmission =
        serde_json::from_value(reply["outcome"].clone()).unwrap();
    let PrivateOriginalSubmission::Admitted {
        turn_ticket: Some(ticket),
        ..
    } = observation
    else {
        panic!("actual P admission")
    };
    await_turn(case, "planning-parent", &ticket).await;
    let selected = planning
        .source
        .prepare_workflow_planning(
            &planning.parent.original,
            planning.lead.clone(),
            planning.terms.clone(),
            planning.lineage.clone(),
        )
        .await
        .unwrap();
    launch.select_planning(selected).unwrap();
    launch.enable_context_transfers(planning.source.clone());
    let make = |tag: &str| {
        let content = format!(
            "sdk-planning-{tag}: read one public Hacker News story using the packaged integration"
        );
        (
            source_event(
                &format!("original-{tag}"),
                &format!("sdk-planning-{tag}"),
                &content,
                vec![],
            ),
            content,
        )
    };
    let (a_event, a_text) = make("a");
    let (b_event, b_text) = make("b");
    // Explicit fresh host selection precedes either first input. The SDK
    // launcher retains these exact same authenticated events idempotently.
    let a = planning.source.retain_event(a_event.clone()).await.unwrap();
    let b = planning.source.retain_event(b_event.clone()).await.unwrap();
    planning
        .member
        .prepare_effect_waiter_source(&a)
        .await
        .unwrap();
    planning
        .member
        .prepare_effect_waiter_source(&b)
        .await
        .unwrap();
    let (actual_a, actual_b) = tokio::join!(
        launch.launch(
            meerkat_workgraph::WorkOwnerKey::agent("sdk-original-a").unwrap(),
            a_event,
            a_text
        ),
        launch.launch(
            meerkat_workgraph::WorkOwnerKey::agent("sdk-original-b").unwrap(),
            b_event,
            b_text
        )
    );
    assert!(actual_a.unwrap().unwrap() == a);
    assert!(actual_b.unwrap().unwrap() == b);
    for row in launch.observations() {
        await_turn(
            case,
            row["original"]["route"].as_str().unwrap(),
            row["ticket"].as_str().unwrap(),
        )
        .await;
    }
    tokio::time::timeout(BOUND, async {
        while !script.b_context_observed.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (workflow, _, _) = workflow_evidence(case);
    assert_eq!(workflow["waiters"].as_object().unwrap().len(), 2);
    assert_eq!(input_count(case, "original-a").await, 1);
    assert_eq!(input_count(case, "original-b").await, 1);
    let a_before = history(case, "original-a").await;
    let cancelled = case
        .client
        .lock()
        .await
        .call(json!({"kind":"cancel_original_operation","original":a}))
        .await
        .unwrap();
    assert_eq!(cancelled["kind"], "original_operation_cancelled");
    let a_key = planning.source.source_record_key(&a).await.unwrap();
    tokio::time::timeout(BOUND, async {
        loop {
            let (workflow, _, _) = workflow_evidence(case);
            let row = workflow["waiters"]
                .as_object()
                .unwrap()
                .values()
                .find(|row| row["identity"]["source"] == json!(a_key))
                .unwrap();
            if row["departed"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("canonical A departure, not merely cancelled observation");
    assert!(!script.issued.lock().unwrap().contains_key("g-install"));
    case.approve_independently();
    script.release_b.store(true, Ordering::Release);
    // The protected parent observes one real request from the same installed
    // endpoint. It grants only B's exact public scope and complete catalog
    // successor; no model text can supply this administrative decision.
    tokio::time::timeout(PUBLIC_BOUND, async {
        loop {
            if script
                .issued
                .lock()
                .unwrap()
                .contains_key("g-public-access")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("actual protected install/reconcile must finish before access");
    let request = case
        .client
        .lock()
        .await
        .call(json!({"kind":"next_service_parent_administration"}))
        .await
        .unwrap();
    assert_eq!(request["kind"], "service_parent_administration_request");
    let proposal: toolkit_mobkit::private_originals::ServiceParentAuthorizationProposal =
        serde_json::from_value(request["proposal"].clone()).unwrap();
    assert!(proposal.original == b);
    assert_eq!(proposal.operator, principal("admin"));
    assert_eq!(proposal.expires_in_ms, Some(180_000));
    assert_eq!(proposal.decision.route, id("original-b"));
    assert_eq!(
        proposal.decision.permission.operations,
        [id(OPERATION)].into()
    );
    let destination = &proposal
        .decision
        .live_attachment
        .as_ref()
        .expect("actual B attachment proposal")
        .destination;
    assert_eq!(
        destination.context.as_str(),
        case.audit_sessions["original-b"].to_string()
    );
    assert_eq!(
        request["proposal_sha256"],
        proposal.proposal_sha256().unwrap()
    );
    let policy = proposal
        .tool_policy
        .as_ref()
        .expect("actual complete successor for B catalog");
    assert_eq!(policy.member, member("original-b").member);
    let submitted=case.client.lock().await.call(json!({"kind":"submit_service_parent_administration",
        "exchange_id":request["exchange_id"],"proposal_sha256":request["proposal_sha256"],"decision":{
            "kind":"authorized_with_tool_policy","source":{"kind":"standing_rule_decision"},
            "authorization_ref":"sdk-public-only-b-exact-proposal","provenance":{"scope":"one actual B public HN operation","proposal_sha256":request["proposal_sha256"]},
            "tool_policy_sha256":policy.sha256().unwrap()}})).await.unwrap();
    assert_eq!(submitted["kind"], "service_parent_administration_submitted");
    let result=case.client.lock().await.call(json!({"kind":"result_service_parent_administration","exchange_id":request["exchange_id"]})).await.unwrap();
    assert_eq!(result["observation"]["custody"], "joined");
    assert_eq!(result["observation"]["outcome"], "succeeded");
    tokio::time::timeout(PUBLIC_BOUND, async {
        loop {
            if script.answer.lock().unwrap().is_some()
                && script
                    .observed
                    .lock()
                    .unwrap()
                    .contains_key("g-public-access")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("real public MCP result and automatic fresh B continuation");
    let runtime = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(
        &case.audit_sessions["original-b"],
    );
    let rows = case
        .audit_store
        .as_ref()
        .unwrap()
        .load_input_states_strict(&runtime)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        2,
        "only original B plus one fresh ordinary continuation"
    );
    let service = case
        .native
        .as_ref()
        .unwrap()
        .mob_runtime()
        .session_service()
        .unwrap();
    let machine = service.acquire_runtime_adapter(None).unwrap().unwrap();
    for row in &rows {
        tokio::time::timeout(BOUND, async {
            loop {
                if machine
                    .input_terminal_completion(
                        &case.audit_sessions["original-b"],
                        &row.state.input_id,
                    )
                    .await
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    let rows = case
        .audit_store
        .as_ref()
        .unwrap()
        .load_input_states_strict(&runtime)
        .await
        .unwrap();
    assert!(rows.iter().all(|row| row.seed.terminal_outcome.is_some()));
    assert_native_call(&rows, "b-public-read", OPERATION);
    let g_runtime = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(
        &case.audit_sessions["planning-lead"],
    );
    let g_rows = tokio::time::timeout(BOUND, async {
        loop {
            let rows = case
                .audit_store
                .as_ref()
                .unwrap()
                .load_input_states_strict(&g_runtime)
                .await
                .unwrap();
            if rows.len() == 3 && rows.iter().all(|row| row.seed.terminal_outcome.is_some()) {
                break rows;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("one G input and both actual contexts must settle");
    assert_native_call(&g_rows, "g-install", "setup_install");
    assert_native_call(
        &g_rows,
        "g-public-access",
        meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST,
    );
    assert_eq!(input_count(case, "original-a").await, 1);
    assert_eq!(history(case, "original-a").await, a_before);
    let b_history = history(case, "original-b").await;
    assert!(
        b_history
            .to_string()
            .contains(script.answer.lock().unwrap().as_ref().unwrap())
    );
    assert_eq!(
        case.approved.lock().unwrap().len(),
        1,
        "one actual software approval belonging to G"
    );
    let reviewed = case.approved.lock().unwrap()[0].clone();
    assert!(
        matches!(&reviewed.owner, meerkat_core::ApprovalOwnerRef::Session {session_id}
        if session_id == &case.audit_sessions["planning-lead"])
    );
    assert_eq!(
        reviewed.resource.kind,
        meerkat_core::ApprovalResourceKind::ToolCall
    );
    assert_eq!(reviewed.resource.id.as_str(), "setup_install");
    let body = reviewed
        .proposed_action
        .body
        .as_ref()
        .expect("exact software proposal");
    assert_eq!(body["call_id"], "g-install");
    assert!(body.to_string().contains("hackernews"));
    let issued = script.issued.lock().unwrap().clone();
    assert_eq!(
        issued
            .values()
            .filter(|row| row["name"] == OPERATION)
            .count(),
        1
    );
    assert_eq!(
        issued
            .values()
            .filter(|row| row["name"] == toolkit_mobkit::FIND_AND_INSTALL)
            .count(),
        2
    );
    let (workflow, effects, acquisitions) = workflow_evidence(case);
    let launches = effects
        .iter()
        .filter(|effect| effect.intent.kind == "setup_workflow_launch")
        .collect::<Vec<_>>();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].status, toolkit_store::EffectStatus::Succeeded);
    let evidence = json!({"scope":"native shared public consumer with controlled model and real HN network",
        "source_admission":launch.observations(),"workflow":workflow,"effects":effects,"acquisitions":acquisitions,
        "service_parent":request,"service_result":result,"software_reviews":*case.approved.lock().unwrap(),
        "model_calls":issued,"actual_results":*script.observed.lock().unwrap(),"answer":*script.answer.lock().unwrap(),
        "b_inputs":rows,"g_inputs":g_rows,"a_history":a_before,"b_history":b_history,
        "first_g_operation_before_promotion":"not exercised by this schedule"});
    fs::write(
        case.directory.path().join("SHARED-PUBLIC-RESULT.json"),
        canonical_json(&evidence).unwrap(),
    )
    .unwrap();
    let retired=case.client.lock().await.call(json!({"kind":"retire_service_parent_administration","exchange_id":request["exchange_id"]})).await.unwrap();
    assert_eq!(retired["observation"]["custody"], "joined");
    for original in [&b, &planning.parent.original] {
        let result = case
            .client
            .lock()
            .await
            .call(json!({"kind":"cancel_original_operation","original":original}))
            .await
            .unwrap();
        assert_eq!(result["kind"], "original_operation_cancelled");
    }
}

fn assert_native_call(
    rows: &[meerkat_runtime::input_state::StoredInputState],
    call: &str,
    name: &str,
) {
    use meerkat_authorization_contracts::audit::{
        AuditObservation, AuditTarget, StoredAuthorizationAuditObservation,
    };
    let audits = rows
        .iter()
        .flat_map(|row| {
            let encoded = serde_json::to_value(row).unwrap();
            serde_json::from_value::<Vec<StoredAuthorizationAuditObservation>>(
                encoded["authorization_audit"].clone(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let ids = audits
        .iter()
        .filter_map(|row| match &row.observation.observation {
            AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Tool {call_id,tool_name,..}
            if call_id.as_str()==call && tool_name.as_str()==name) =>
            {
                Some(row.observation.operation_id.to_string())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids.len(),
        1,
        "one actual native authorization for {call}/{name}"
    );
    let id = ids.iter().next().unwrap();
    assert!(
        audits
            .iter()
            .filter(|row| row.observation.operation_id.to_string() == *id)
            .any(|row| matches!(
                row.observation.observation,
                AuditObservation::Entry {
                    boundary: Some(meerkat_core::OperationEntryBoundary::ToolEffect)
                }
            )),
        "actual native effect entry, not only a model-generated answer"
    );
}

#[test]
#[ignore = "requires current full packaged resources and one real public Hacker News read"]
fn shared_public_native_install_survives_one_waiter_departure() {
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(Box::pin(async move {
        let mut case = Case::new(parent.enter());
        let script = Arc::new(PublicScript::default());
        let slots = Arc::new(Mutex::new(None));
        let prepared_runtime = Arc::new(Mutex::new(None));
        let mut remote = None;
        let mut retained_host = None;
        let checked = std::panic::AssertUnwindSafe(async {
            public_construction::construct(
                &mut case,
                pipes,
                slots.clone(),
                prepared_runtime.clone(),
                script.clone(),
            )
            .await;
            let mut constructed = slots.lock().unwrap().take().unwrap();
            retained_host = constructed.host.clone();
            case.private = constructed.private.take();
            case.launch = Some(constructed.launch.clone());
            let private = case.private.as_ref().unwrap();
            remote = Some(tokio::spawn(
                crate::application_remote::serve_remote_operator_with_service_parent(
                    constructed.host.as_ref().unwrap().clone(),
                    constructed.remote.take().unwrap(),
                    case.directory.path().into(),
                    private.closed_observer(),
                    private.service_parent_endpoint().unwrap(),
                ),
            ));
            exercise(&mut case, &mut constructed, &script).await;
            drop(constructed);
        })
        .catch_unwind()
        .await;
        // Native Abort may occur after private adoption. Drain that same
        // retained connection instead of losing it with the held observer.
        if let Some(mut pending) = slots.lock().unwrap().take() {
            retained_host = pending.host.clone();
            case.private = pending.private.take();
            case.launch = Some(pending.launch.clone());
            drop(pending.remote.take());
            drop(pending);
        }
        let host_closing = retained_host.as_ref().map(|host| {
            host.begin_close(meerkat_mobkit::rpc_gateway::GatewayHostCloseReason::Shutdown)
        });
        if checked.is_err() {
            case.retain_failed_audits().await;
        }
        // This is the actual prepared participant, retained before bootstrap.
        // It may own custody recovery even if final private adoption failed.
        let prepared_runtime = prepared_runtime.lock().unwrap().take();
        if let Some(prepared) = &prepared_runtime {
            prepared.begin_custody_close();
        }
        let resource = case.resource.take();
        // Native/private cleanup and the gateway's actual recovery join both
        // run even when construction, publication or close entry failed.
        let (mut cleanup, host_join) = tokio::join!(case.cleanup(), async {
            match &retained_host {
                Some(host) => host.join_close().await,
                None => Ok(()),
            }
        });
        if let Some(Err(error)) = host_closing {
            cleanup.push(format!("prepared gateway close: {error}"));
        }
        if let Err(error) = host_join {
            cleanup.push(format!("prepared gateway join: {error}"));
        }
        drop(retained_host);
        // Match the normal driver order: stop before shutdown, join the same
        // entered source task after private/native transport shutdown.
        if let Some(prepared) = prepared_runtime
            && let Err(error) = prepared.join_custody_close().await
        {
            cleanup.push(format!("prepared custody recovery join: {error}"));
        }
        if let Some(remote) = remote {
            match remote.await {
                Ok(Ok(())) => (),
                other => cleanup.push(format!("protected operator join: {other:?}")),
            }
        }
        if let Some(owner) = resource {
            if owner.attempts.get().is_some() {
                match owner
                    .join_attempt_guards(tokio::time::Instant::now() + BOUND)
                    .await
                {
                    Ok(report) if report.is_complete() => (),
                    other => cleanup.push(format!("actual MCP attempts not joined: {other:?}")),
                }
            }
            if let Err(error) = owner.close().await {
                cleanup.push(error.to_string());
            }
        }
        fs::write(
            case.directory.path().join("SHARED-PUBLIC-CLEANUP.json"),
            canonical_json(&json!({"failures":cleanup})).unwrap(),
        )
        .unwrap();
        let retained = case.directory.keep();
        eprintln!("shared_public_native_evidence={}", retained.display());
        assert!(
            cleanup.is_empty(),
            "actual owned cleanup must settle: {cleanup:?}"
        );
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    }));
}
