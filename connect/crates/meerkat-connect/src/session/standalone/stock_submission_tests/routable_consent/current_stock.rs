//! Complementary stock acceptance only. The two actual R3 calls belong to one
//! current run; this does not satisfy the three-origin cold census in case205.
use super::*;

#[tokio::test]
async fn latest_stock_run_admits_two_distinct_approval_notices_without_another_prompt() {
    let parent = tempfile::tempdir().unwrap();
    let parent_root = parent.path().canonicalize().unwrap();
    let client = Arc::new(ScriptedProvider::default());
    let observed = client.clone();
    let peer = ModelPeer::open(
        MODEL,
        Arc::new(move |body| Box::pin(std::future::ready(observed.reply(body)))),
    )
    .await
    .unwrap();
    let mut setup: Option<Opened> = None;
    let mut directory = None;
    let mut runtime: Option<Native> = None;
    let mut active: Option<ConnectSession> = None;
    let mut issuer_owner = None;
    let caught = AssertUnwindSafe(async {
        let peer_source = format!("printf '%s\\n' \"$$\" >> \"$1.pids\"\n{MCP_PEER}");
        setup = Some(Opened::open_with_selected_mcp_in(None, &peer, client.clone(),
            &peer_source, Some(&parent_root)).await.unwrap());
        let opened = setup.as_ref().unwrap();
        let selected = opened.continuation.storage.clone();
        let id = opened.continuation.session_id.clone();
        let server = opened.continuation.server.clone();
        setup.as_mut().unwrap().finish().await.unwrap();
        directory = Some(release_setup(setup.take().unwrap()));
        let root = directory.as_ref().unwrap().path().canonicalize().unwrap();
        let config_path = crate::realm::config_source(&selected).unwrap()
            .config_doc_path(&selected.locator.realm);
        let mut config = std::fs::read_to_string(&config_path).unwrap();
        config.push_str("\n[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"toolkit-setup\" }\ntier = \"r3\"\nconsent_assurance = \"person_authenticated\"\n");
        std::fs::write(config_path, config).unwrap();
        let owner = ApplicationConsentOwner::create(&root,
            issuer::identity_for_realm(selected.locator.realm.clone())).unwrap();
        issuer_owner = Some(owner.clone());
        runtime = Some(Native::open(&selected, &id, &owner, &server).await.unwrap());
        let native = runtime.as_mut().unwrap();
        active = Some(native.attach(&selected, &server).await.unwrap());
        let claim = native.machine.grant_session_hosting(&id).unwrap();
        assert!(claim.is_cross_process());
        let association = owner.bind_association(issuer::artifact(&root.join("association.json"),
            b"{\"fixture\":\"two actual R3 calls in one current stock run\"}"), &claim).unwrap();
        let before_inputs: HashSet<_> = native.store.load_input_states_strict(&LogicalRuntimeId::for_session(&id))
            .await.unwrap().into_iter().map(|row| row.state.input_id).collect();
        for call in ["current-stock-first", "current-stock-second"] {
            client.1.lock().unwrap().push_back((call.into(), "setup_help".into(), serde_json::json!({})));
        }
        // One native input and run. The second model request follows the first
        // real consent refusal in that run, without rebinding its destination.
        let (accepted, completion) = active.as_ref().unwrap()
            .submit("Request both separately gated operations in this run".into()).await.unwrap();
        let AcceptOutcome::Accepted { input_id, .. } = accepted else { panic!("actual stock admission required") };
        let (outcome, terminal) = completion.try_wait_with_terminal_outcome().await.unwrap().into_parts();
        assert!(matches!(outcome, CompletionOutcome::Completed(_)));
        assert_eq!(terminal, Some(InputTerminalOutcome::Consumed));
        let rows = scoped(&owner, association.scope());
        assert_eq!(rows.len(), 2, "both calls must actually park native consent");
        assert_ne!(rows[0].approval_id, rows[1].approval_id);
        {
            let observed = client.2.lock().unwrap();
            let mut requested = HashSet::new();
            for call in ["current-stock-first", "current-stock-second"] {
                let result = observed.get(call).expect("native model received each real call refusal");
                assert!(result.is_error);
                let refusal: serde_json::Value = serde_json::from_str(&result.text).unwrap();
                assert_eq!(refusal["error"], "review_unsatisfied");
                assert_eq!(refusal["data"]["kind"], "human_consent_required");
                let approval = refusal["data"]["approval_id"].as_str().unwrap();
                assert!(rows.iter().any(|row| row.approval_id.to_string() == approval));
                assert!(requested.insert(approval.to_owned()));
            }
            assert_eq!(requested.len(), 2, "two distinct approvals returned by the actual call refusals");
        }
        let work = &rows[0].consent.as_ref().unwrap().origin.as_ref().unwrap().retained_work;
        for row in &rows {
            let consent = row.consent.as_ref().unwrap();
            assert_eq!(row.status, ApprovalStatus::Pending);
            assert!(consent.routable() && consent.withdrawal.is_none() && consent.consumed_at.is_none());
            assert!(matches!(consent.custody, Some(ApprovalConsentCustody::Parked)));
            assert!(consent.deliveries.is_empty());
            let current = &consent.origin.as_ref().unwrap().retained_work;
            assert_eq!(current.runtime_id(), LogicalRuntimeId::for_session(&id).0.as_str());
            assert_eq!(current.run_id(), work.run_id());
            assert_eq!(current.contributors().len(), 1);
            assert_eq!(current.contributors()[0].input_id, input_id);
        }
        let actual = native.machine.input_terminal_receipt_observation(&id,
            InteractionSelector::InputId(input_id.clone())).await.unwrap().unwrap();
        assert_eq!(actual.source, TerminalWitnessSource::DurableStore);
        let InputTerminalReceiptObservation::Finalized(receipt) = actual.report else { panic!("actual original terminal") };
        assert_eq!(receipt.run_id(), Some(work.run_id()));
        let held = native.machine.hold_run_starts(&id, RunStartHoldReason::MobStop).await.unwrap();
        assert!(held.current_run.is_none());
        let requests = client.0.load(Ordering::SeqCst);
        for row in &rows { issuer::approve(&owner, &row.approval_id); }
        native.arm();
        let delivered = wait_rows(&owner, association.scope(), native.delivery.as_ref().unwrap(), |rows|
            rows.len() == 2 && delivery_ids(rows, ApprovalConsentTransition::Approved).len() == 2).await;
        let notices = delivery_ids(&delivered, ApprovalConsentTransition::Approved);
        assert_eq!(notices.iter().collect::<HashSet<_>>().len(), 2);
        assert!(notices.iter().all(|notice| notice != &input_id));
        let stored = native.store.load_input_states_strict(&LogicalRuntimeId::for_session(&id)).await.unwrap();
        for notice in &notices {
            let row = stored.iter().find(|row| &row.state.input_id == notice).unwrap();
            assert_eq!(row.seed.phase, meerkat_runtime::input_state::InputLifecycleState::Queued);
            assert!(row.seed.terminal_outcome.is_none());
        }
        let expected_new: HashSet<_> = std::iter::once(input_id.clone()).chain(notices.iter().cloned()).collect();
        let actual_new: HashSet<_> = stored.iter().map(|row| row.state.input_id.clone())
            .filter(|id| !before_inputs.contains(id)).collect();
        assert_eq!(actual_new, expected_new, "one original and exactly two notice admissions");
        assert_eq!(client.0.load(Ordering::SeqCst), requests, "queued notices must not run a model");
        assert!(!root.join("unexpected-mcp-calls").exists(), "no MCP operation entered");
        native.retire_delivery().await.unwrap();
        assert!(native.delivery.is_none() && native.retired.is_some());
        active.take().unwrap().close().await.unwrap();
        let expected: HashSet<_> = std::iter::once(input_id).chain(notices).collect();
        assert_eq!(expected.len(), 3);
        durable_inputs(native, &expected).await;
        assert_eq!(client.0.load(Ordering::SeqCst), requests);
        assert!(!root.join("unexpected-mcp-calls").exists());
    }).catch_unwind().await;
    let mut failures = Vec::new();
    if let Some(native) = runtime.as_mut()
        && native.retire_delivery().await.is_err()
    {
        failures.push("delivery retirement");
    }
    if let Some(session) = active.take()
        && session.close().await.is_err()
    {
        failures.push("actual session close");
    }
    if let Some(native) = runtime.as_mut()
        && native.close().await.is_err()
    {
        failures.push("actual native close");
    }
    if let Some(opened) = setup.as_mut()
        && opened.finish().await.is_err()
    {
        failures.push("staged setup close");
    }
    if peer.finish().await.is_err() {
        failures.push("model peer join");
    }
    drop((runtime, issuer_owner));
    match std::fs::read_dir(&parent_root) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) if entry.path().is_dir() => {
                        if helpers_gone(&entry.path()).await.is_err() {
                            failures.push("native helper liveness");
                        }
                    }
                    Ok(_) => {}
                    Err(_) => failures.push("helper root observation"),
                }
            }
        }
        Err(_) => failures.push("helper root census"),
    }
    if caught.is_err() || !failures.is_empty() {
        if let Some(directory) = directory.take() {
            let _retained = directory.keep();
        }
        let _retained = parent.keep();
    }
    if !failures.is_empty() {
        let _ = writeln!(
            std::io::stderr(),
            "stock two-notice cleanup failures: {failures:?}"
        );
    }
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    assert!(failures.is_empty(), "{failures:?}");
}
