//! Real class Add and native bound inputs. Only model transport is scripted.
//! Host-original outbound controls stay separate from the receiver-owned
//! Peer admission and reply controls in the sibling module.
use super::*;
use crate::private_originals::{
    NativeOriginalOperationsRequest, NativeOriginalTarget, NativeOriginalTargetRequest,
    NativePeerAction, NativeRecorderAction,
};
use meerkat_authorization_contracts::{
    audit::{AuditObservation, AuditTarget, StoredAuthorizationAuditObservation},
    work_association::GrantTarget,
};
use meerkat_core::authorization::OperationObservedOutcome;

pub(super) async fn native_input_states(
    fixture: &Fixture,
    session: &meerkat_core::SessionId,
) -> Vec<meerkat_runtime::input_state::StoredInputState> {
    // Read every committed native input, including terminal rows, from the
    // exact runtime store shared by this service and machine.
    let store = fixture.service.runtime_store();
    assert!(fixture.machine.shares_runtime_store_authority(&store));
    store
        .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(session))
        .await
        .unwrap()
}

async fn start() -> Fixture {
    start_with_policy_store(None).await
}

async fn start_with_policy_store(decorate: Option<installed::PrepareOriginalSource>) -> Fixture {
    Fixture::start_selected_source_with_recorder(native_class(), decorate, None, true).await
}

fn native_class() -> CompiledMemberClass {
    let mut selected = class();
    for name in ["memory", "peers", "send_message", "send_request"] {
        selected.grants.push(CompiledMemberToolGrant {
            tool_name: name.into(),
            action: CompiledMemberToolAction::Invoke,
            consequence: if name == "peers" {
                CompiledToolConsequence::R0
            } else {
                CompiledToolConsequence::R1
            },
        });
    }
    selected
        .grants
        .sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
    selected
}

pub(super) async fn add(fixture: &Fixture, name: &str) -> meerkat_core::SessionId {
    let admitted = fixture.admit(name).await;
    let context = fixture.native.identity_first_context().unwrap();
    let desired = spec(name, &class().profile);
    let publish = desired.clone();
    let roster = fixture.roster.clone();
    let started = fixture
        .gateway
        .add_member_class(
            &context,
            admitted,
            desired,
            fixture.route(name),
            fixture.intent(name),
            tokio::time::Instant::now() + WAIT,
            move || async move {
                roster.upsert(publish);
                Ok(())
            },
        )
        .await
        .unwrap();
    let outcome = match started {
        MemberClassAddStart::Settled(outcome) => outcome,
        MemberClassAddStart::Unsettled(settlement) => settlement.settled().await.unwrap(),
    };
    assert!(
        outcome.authorization.is_ok() && outcome.cleanup.is_ok(),
        "{outcome:?}"
    );
    assert!(matches!(
        outcome
            .native
            .unwrap()
            .outcomes
            .get(&AgentIdentity::parse(name).unwrap()),
        Some(RestoreOutcome::Created { .. })
    ));
    fixture
        .native
        .identity_runtime()
        .unwrap()
        .status(&AgentIdentity::parse(name).unwrap())
        .await
        .unwrap()
        .session_id
        .unwrap()
}

pub(super) async fn ready(fixture: &Fixture, session: &meerkat_core::SessionId) {
    let lease = fixture
        .service
        .acquire_live_session_actor_turn_boundary_lease(session)
        .await
        .unwrap();
    let pin = fixture
        .service
        .pin_controller_client_for_actor(&lease)
        .await
        .unwrap();
    drop(lease);
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &fixture.machine.generated_auth_lease_handle(),
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
        tokio::time::Instant::now() + WAIT,
    )
    .await
    .unwrap();
}

async fn original(fixture: &Fixture, label: &str) -> PrivateOriginalSubmit {
    let mut event = private_event(label);
    event.route = fixture.route(ADDED).id;
    event.content_sha256 = digest(PROMPT.as_bytes());
    let transport_authentication = event.original_authentication.clone();
    let original = fixture.source.retain_event(event).await.unwrap();
    PrivateOriginalSubmit {
        original,
        content: PROMPT.into(),
        transport_authentication,
    }
}

async fn selection(fixture: &Fixture, label: &str) -> NativeOriginalOperationsRequest {
    let now = u64::try_from(fixture.source.store.now_ms().await.unwrap()).unwrap();
    NativeOriginalOperationsRequest {
        not_before_ms: now,
        expires_at_ms: now + 120_000,
        targets: [
            (
                "peer",
                NativeOriginalTarget::Peer {
                    route: fixture.route("project-aborted").id,
                    actions: vec![NativePeerAction::Message, NativePeerAction::Request],
                },
            ),
            ("directory", NativeOriginalTarget::PeerDirectory),
            (
                "recorder",
                NativeOriginalTarget::RecorderIdentity {
                    actions: vec![
                        NativeRecorderAction::Read,
                        NativeRecorderAction::Write,
                        NativeRecorderAction::Delete,
                    ],
                },
            ),
        ]
        .into_iter()
        .map(|(kind, target)| NativeOriginalTargetRequest {
            issuance_id: EvidenceId::new(format!("{label}-{kind}")).unwrap(),
            target,
        })
        .collect(),
    }
}

async fn run(
    fixture: &Fixture,
    session: &meerkat_core::SessionId,
    submission: PrivateOriginalSubmit,
    tool: &'static str,
    args: serde_json::Value,
    success: Option<bool>,
) -> String {
    *fixture.factory.next_call.lock().unwrap() = Some(tool);
    *fixture.factory.next_args.lock().unwrap() = args;
    fixture.factory.callback_results.lock().unwrap().clear();
    let submitted = fixture
        .source
        .submit_authenticated_original(submission)
        .await
        .unwrap();
    let PrivateOriginalSubmission::Admitted {
        input_id,
        turn_ticket: Some(ticket),
        ..
    } = submitted
    else {
        panic!("selected ordinary input was not admitted");
    };
    let outcome = fixture
        .native
        .identity_runtime()
        .unwrap()
        .wait_for_turn(
            &AgentIdentity::parse(ADDED).unwrap(),
            TurnTicket::parse(&ticket).unwrap(),
            WAIT,
        )
        .await;
    assert!(
        matches!(outcome, TurnOutcome::Completed { output: TurnOutput::Text { text, .. } } if text == ANSWER)
    );
    let native_input = meerkat_core::InputId::from_uuid(uuid::Uuid::parse_str(&input_id).unwrap());
    // Read the same canonical store in both fixture modes. Durable terminal
    // inputs can already have left the machine's live driver ledger.
    let stored = fixture
        .runtime_store
        .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(session))
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.state.input_id == native_input)
        .expect("actual native input history");
    let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
        serde_json::to_value(stored).unwrap()["authorization_audit"].clone(),
    )
    .unwrap();
    let ids = audit.iter().filter_map(|row| match &row.observation.observation {
        AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Tool { tool_name, .. } if tool_name == tool)
            => Some(row.observation.operation_id.clone()),
        _ => None,
    }).collect::<std::collections::HashSet<_>>();
    let returned = audit.iter().any(|row| {
        ids.contains(&row.observation.operation_id)
            && matches!(
                &row.observation.observation,
                AuditObservation::Outcome {
                    outcome: OperationObservedOutcome::ToolDispatchReturned {
                        result_is_error: false,
                        terminal_error: None,
                        ..
                    }
                }
            )
    });
    if let Some(success) = success {
        assert_eq!(
            returned, success,
            "the actual native operation must match its selected authority"
        );
    } else {
        assert_eq!(
            ids.len(),
            1,
            "one actual selected peer operation must be prepared"
        );
        assert!(
            audit
                .iter()
                .any(|row| ids.contains(&row.observation.operation_id)
                    && matches!(
                        &row.observation.observation,
                        AuditObservation::Entry {
                            boundary: Some(
                                meerkat_core::OperationEntryBoundary::ToolExecutionPolicy
                            ),
                        }
                    )),
            "actual sender policy must enter; projection alone is insufficient"
        );
        assert!(
            audit
                .iter()
                .any(|row| ids.contains(&row.observation.operation_id)
                    && matches!(
                        &row.observation.observation,
                        AuditObservation::Outcome { .. }
                    )),
            "the entered native tool must return its real disposition"
        );
        assert!(
            audit
                .iter()
                .all(|row| !ids.contains(&row.observation.operation_id)
                    || !matches!(
                        &row.observation.observation,
                        AuditObservation::Refused { .. }
                            | AuditObservation::AuthorizationUnavailable { .. }
                    )),
            "sender authorization must not refuse the selected target"
        );
    }
    if success == Some(false) {
        assert!(audit.iter().any(|row| matches!(&row.observation.observation,
            AuditObservation::Refused { target, reason: meerkat_core::authorization::OperationRefusalKind::Denied, .. }
                if matches!(target.as_ref(), AuditTarget::Tool { tool_name, .. } if tool_name == tool))),
            "missing dispatch is insufficient: require native denied evidence");
    }
    input_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_target_preparation_restores_exact_grants_before_first_input() {
    let fixture = Box::pin(Fixture::start_durable_with_recorder(native_class())).await;
    let prepared = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 6,
        Box::pin(async {
            let f = &fixture;
            let session = add(f, ADDED).await;
            let recipient_session = add(f, "project-aborted").await;
            ready(f, &session).await;
            let recipient = f
                .policy
                .member_class_admission(&instance("project-aborted").member.member)
                .unwrap()
                .unwrap();
            let caller = f
                .policy
                .member_class_admission(&instance(ADDED).member.member)
                .unwrap()
                .unwrap();
            for member in [&caller, &recipient] {
                let surface = member
                    .native_surface
                    .as_ref()
                    .expect("actual native Add surface");
                for name in [CALLBACK, OTHER_CALLBACK, "memory", "peers"] {
                    assert!(
                        surface.iter().any(|tool| tool.name.as_str() == name),
                        "the exact surface retained across Restore must include {name}"
                    );
                }
            }
            let mut saved = Vec::new();
            for label in ["cold-directory", "cold-recorder", "cold-retired-recipient"] {
                let submission = original(f, label).await;
                let request = selection(f, label).await;
                f.source
                    .prepare_original_native_operations(submission.clone(), request.clone())
                    .await
                    .unwrap();
                let key = f.source.key(&submission.original).unwrap();
                let row = f.source.store.get(&key).await.unwrap().unwrap();
                let record = f.source.decode_record(&row, &submission.original).unwrap();
                assert!(record.final_association.is_none());
                let basis = if label == "cold-retired-recipient" {
                    None
                } else {
                    Some(
                        f.source
                            .native_operations_basis(
                                &submission.original,
                                &record,
                                &session,
                                WorkAuthorityBasis::TargetedGrantLineages {
                                    base: None,
                                    targets: vec![],
                                },
                            )
                            .await
                            .unwrap(),
                    )
                };
                assert_eq!(f.source.store.get(&key).await.unwrap(), Some(row.clone()));
                saved.push((submission, request, key, row, basis));
            }
            for selected in [&session, &recipient_session] {
                assert!(
                    f.runtime_store
                        .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(
                            selected
                        ))
                        .await
                        .unwrap()
                        .is_empty(),
                    "preparation and issuance cannot submit input"
                );
            }
            assert_eq!(
                f.roster.snapshot().len(),
                3,
                "the actual Add finalizers published both desired members"
            );
            (saved, session, recipient_session, caller, recipient)
        }),
    ))
    .catch_unwind()
    .await;
    let (saved, session, recipient_session, caller, recipient) = match prepared {
        Ok(Ok(prepared)) => prepared,
        failed => {
            fixture.close().await;
            match failed {
                Err(panic) => std::panic::resume_unwind(panic),
                _ => panic!("cold native target preparation did not settle"),
            }
        }
    };
    let old_source = Arc::downgrade(&fixture.source);
    let old_lifetime = Arc::downgrade(&fixture.lifetime);
    let old_factory = Arc::downgrade(&fixture.factory);
    // Join old-owner close outside any observation timeout before reopening.
    let fixture = fixture.reopen_with_recorder().await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 6, Box::pin(async {
        let f = &fixture;
        assert!(old_source.upgrade().is_none() && old_lifetime.upgrade().is_none() && old_factory.upgrade().is_none(),
            "the old source, host and executable callback owners must be dropped");
        assert_eq!(f.policy.member_class_admission(&caller.instance.member.member).unwrap(), Some(caller));
        assert_eq!(f.policy.member_class_admission(&recipient.instance.member.member).unwrap(), Some(recipient.clone()));
        for (name, expected) in [(ADDED, &session), ("project-aborted", &recipient_session)] {
            let status = f.native.identity_runtime().unwrap().status(&AgentIdentity::parse(name).unwrap()).await.unwrap();
            assert_eq!(status.session_id.as_ref(), Some(expected));
            assert_eq!(status.state, meerkat_mobkit::identity_first::IdentityLifecycleState::Active);
            assert!(f.runtime_store.load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(expected))
                .await.unwrap().is_empty(), "Restore must not submit an original");
        }
        ready(f, &session).await;
        let recipient_entry = f.native.mob_handle().get_member(
            &meerkat_mobkit::member_comms_id::mob_member_id("project-aborted")).await.unwrap().unwrap();
        let peer = recipient_entry.peer_id().expect("restored recipient has its actual native peer identity");
        for (submission, request, key, row, basis) in &saved {
            assert_eq!(f.source.store.get(key).await.unwrap().as_ref(), Some(row));
            f.source.prepare_original_native_operations(submission.clone(), request.clone()).await.unwrap();
            assert_eq!(f.source.store.get(key).await.unwrap().as_ref(), Some(row));
            if let Some(basis) = basis {
                assert!(basis.lineage_for(Some(&GrantTarget::Peer { peer })).is_some(),
                    "the retained peer target must match the actual restored recipient key");
                let record = f.source.decode_record(row, &submission.original).unwrap();
                let restored = f.source.native_operations_basis(&submission.original, &record, &session,
                    WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] }).await.unwrap();
                assert_eq!(&restored, basis, "reopen must reuse the exact native root and child IDs");
            }
        }
        let mut changed = saved[0].1.clone();
        let NativeOriginalTarget::Peer { route, .. } = &mut changed.targets[0].target else { unreachable!() };
        *route = f.route(ADDED).id;
        assert!(matches!(f.source.prepare_original_native_operations(saved[0].0.clone(), changed).await,
            Err(PrivateOriginalError::Conflict)), "a saved request cannot select a different recipient");
        assert_eq!(f.source.store.get(&saved[0].2).await.unwrap().as_ref(), Some(&saved[0].3));
        let mut admitted = Vec::new();
        for (index, tool, args) in [
            (0, "peers", serde_json::json!({})),
            (1, "memory", serde_json::json!({"action":"remember","title":"Cold selected original","body":"retained native recorder selection"})),
        ] {
            admitted.push(run(f, &session, saved[index].0.clone(), tool, args, Some(true)).await);
            let association = f.source.binding_evidence(&saved[index].0.original).await.unwrap().unwrap();
            let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } = saved[index].4.as_ref().unwrap()
                else { panic!("selected target-only native authority"); };
            for target in targets {
                assert_eq!(association.candidate().authority_basis.lineage_for(Some(&target.target)), Some(target.lineage.as_slice()));
            }
        }
        let memory = meerkat_mobkit::memory::SqliteAgentMemoryStore::open(f._directory.path().join("selected-recorder")).unwrap();
        use meerkat_mobkit::identity_first::agent_memory::{AgentMemoryProvider, AgentMemorySelection, AgentMemoryRecallRequest};
        let records = memory.recall(AgentMemoryRecallRequest {
            realm: f.lifetime.selected_policy().native_host.as_ref().unwrap().realm.to_string(),
            identity: AgentIdentity::parse(ADDED).unwrap(), query_text: None, query_terms: vec![],
            selection: AgentMemorySelection::Always, max_entries: 16,
        }).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].body, "retained native recorder selection");
        f.policy.retire_member_class(&recipient.id, &recipient.instance.member.member).await.unwrap();
        let (pending, request, key, row, _) = &saved[2];
        assert!(matches!(f.source.prepare_original_native_operations(pending.clone(), request.clone()).await,
            Err(PrivateOriginalError::Denied)), "the old recipient admission is no longer current");
        assert_eq!(f.source.store.get(key).await.unwrap().as_ref(), Some(row));
        assert!(matches!(f.source.submit_authenticated_original(pending.clone()).await,
            Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)));
        assert!(f.source.binding_evidence(&pending.original).await.unwrap().is_none());
        for target in &request.targets {
            assert!(f.source.owner.grants().issued_reference(&target.issuance_id).unwrap().is_none());
        }
        let mut history = f.runtime_store.load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(&session))
            .await.unwrap().into_iter().map(|row| row.state.input_id.to_string()).collect::<Vec<_>>();
        history.sort(); admitted.sort();
        assert_eq!(history, admitted, "only the two explicit post-Restore inputs may exist");
        assert!(f.runtime_store.load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(&recipient_session))
            .await.unwrap().is_empty(), "this test makes no peer delivery or reply claim");
    }))).catch_unwind().await;
    fixture.close().await;
    match result {
        Ok(result) => result.expect("cold native target preparation did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn added_original_selects_native_directory_and_own_recorder_with_exact_replay() {
    let fixture = Box::pin(start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, Box::pin(async {
        let session = add(&fixture, ADDED).await;
        add(&fixture, "project-aborted").await;
        ready(&fixture, &session).await;
        *fixture.factory.next_call.lock().unwrap() = None;
        let first = original(&fixture, "native-directory").await;
        let request = selection(&fixture, "native-directory").await;
        fixture.source.prepare_original_native_operations(first.clone(), request.clone()).await.unwrap();
        let key = fixture.source.key(&first.original).unwrap();
        let prepared = fixture.source.store.get(&key).await.unwrap().unwrap();
        assert!(fixture.machine.session_input_states(&session).await.unwrap().report.is_empty());
        assert!(fixture.source.decode_record(&prepared, &first.original).unwrap().final_association.is_none());
        fixture.source.prepare_original_native_operations(first.clone(), request).await.unwrap();
        assert_eq!(fixture.source.store.get(&key).await.unwrap(), Some(prepared));
        let input_id = run(&fixture, &session, first.clone(), "peers", serde_json::json!({}), Some(true)).await;
        let before = fixture.source.binding_evidence(&first.original).await.unwrap().unwrap();
        let WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } = &before.candidate().authority_basis
            else { panic!("native targets must not become a base grant"); };
        assert_eq!(targets.iter().filter(|row| matches!(row.target, GrantTarget::Peer { .. } | GrantTarget::PeerDirectory | GrantTarget::Memory { .. })).count(), 3);
        let replay = fixture.source.submit_authenticated_original(first).await.unwrap();
        assert!(matches!(replay, PrivateOriginalSubmission::Admitted { input_id: same, deduplicated: true, .. } if same == input_id));
        assert_eq!(fixture.machine.session_input_states(&session).await.unwrap().report.len(), 1);
        // Actual Meta Recorder, with its identity-pinned scope and real store.
        for (label,args) in [
            ("native-remember", serde_json::json!({"action":"remember","title":"Native selection","body":"selected native recorder body"})),
            ("native-recall", serde_json::json!({"action":"recall","query_text":"selected native recorder body"})),
        ] {
            let submission = original(&fixture, label).await;
            fixture.source.prepare_original_native_operations(submission.clone(), selection(&fixture, label).await).await.unwrap();
            run(&fixture, &session, submission, "memory", args, Some(true)).await;
        }
        let memory = meerkat_mobkit::memory::SqliteAgentMemoryStore::open(fixture._directory.path().join("selected-recorder")).unwrap();
        use meerkat_mobkit::identity_first::agent_memory::{AgentMemoryProvider, AgentMemorySelection, AgentMemoryRecallRequest};
        let read = AgentMemoryRecallRequest {
            realm: fixture.lifetime.selected_policy().native_host.as_ref().unwrap().realm.to_string(),
            identity: AgentIdentity::parse(ADDED).unwrap(), query_text: None, query_terms: vec![],
            selection: AgentMemorySelection::Always, max_entries: 16,
        };
        let records = memory.recall(read).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].body, "selected native recorder body");
        let submission = original(&fixture, "native-forget").await;
        fixture.source.prepare_original_native_operations(submission.clone(), selection(&fixture, "native-forget").await).await.unwrap();
        run(&fixture, &session, submission, "memory", serde_json::json!({"action":"forget","memory_id":records[0].memory_id}), Some(true)).await;
        assert!(memory.recall(AgentMemoryRecallRequest {
            realm: fixture.lifetime.selected_policy().native_host.as_ref().unwrap().realm.to_string(),
            identity: AgentIdentity::parse(ADDED).unwrap(), query_text: None, query_terms: vec![],
            selection: AgentMemorySelection::Always, max_entries: 16,
        }).await.unwrap().is_empty(), "native forget must remove the active authored record");
        assert_eq!(fixture.machine.session_input_states(&session).await.unwrap().report.len(), 4);
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 8);
        assert!(fixture.factory.calls.lock().unwrap().is_empty(), "native tools must not become application callbacks");
    }))).catch_unwind().await;
    fixture.close().await;
    match result {
        Ok(result) => result.expect("native selections did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_peer_message_and_request_reach_native_sender_policy() {
    let fixture = Box::pin(start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 6, Box::pin(async {
        let session = add(&fixture, ADDED).await;
        add(&fixture, "project-aborted").await;
        ready(&fixture, &session).await;
        let entry = fixture.native.mob_handle().get_member(
            &meerkat_mobkit::member_comms_id::mob_member_id("project-aborted")).await.unwrap().unwrap();
        let peer = entry.peer_id().unwrap();
        // Real native target parsing and prepared/entered policy. The native
        // comms owner still decides trust and delivery; this is no receiver or
        // reply qualification. No fabricated callbacks stand in for comms.
        for (label,tool,args) in [
            ("native-message", "send_message", serde_json::json!({"peer_id":peer,"body":"selected native message","handling_mode":"queue"})),
            ("native-request", "send_request", serde_json::json!({"peer_id":peer,"intent":"checksum_token","params":{"subject":"selected native request"},"handling_mode":"queue"})),
        ] {
            let submission = original(&fixture, label).await;
            fixture.source.prepare_original_native_operations(submission.clone(), selection(&fixture, label).await).await.unwrap();
            run(&fixture, &session, submission, tool, args, None).await;
        }
        assert_eq!(fixture.machine.session_input_states(&session).await.unwrap().report.len(), 2);
        assert!(fixture.factory.calls.lock().unwrap().is_empty());
    }))).catch_unwind().await;
    fixture.close().await;
    match result {
        Ok(result) => result.expect("selected sender operations did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn added_original_refuses_absent_and_foreign_native_targets() {
    let fixture = Box::pin(start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 6, Box::pin(async {
        let session = add(&fixture, ADDED).await;
        add(&fixture, "project-aborted").await;
        add(&fixture, "project-refused").await;
        ready(&fixture, &session).await;
        let absent = original(&fixture, "native-no-selection").await;
        run(&fixture, &session, absent, "peers", serde_json::json!({}), Some(false)).await;
        let foreign = original(&fixture, "native-wrong-peer").await;
        fixture.source.prepare_original_native_operations(foreign.clone(), selection(&fixture, "native-wrong-peer").await).await.unwrap();
        let member = fixture.native.mob_handle().get_member(&meerkat_mobkit::member_comms_id::mob_member_id("project-refused")).await.unwrap().unwrap();
        run(&fixture, &session, foreign, "send_message", serde_json::json!({"peer_id":member.peer_id().unwrap(),"body":"must not deliver","handling_mode":"queue"}), Some(false)).await;
        let mob = original(&fixture, "native-mob-not-selected").await;
        fixture.source.prepare_original_native_operations(mob.clone(), selection(&fixture, "native-mob-not-selected").await).await.unwrap();
        run(&fixture, &session, mob, "memory", serde_json::json!({"action":"propose_to_mob","title":"Refused","body":"must not write"}), Some(false)).await;
        assert_eq!(fixture.machine.session_input_states(&session).await.unwrap().report.len(), 3);
        let other = fixture.native.identity_runtime().unwrap().status(&AgentIdentity::parse("project-refused").unwrap()).await.unwrap().session_id.unwrap();
        assert!(fixture.machine.session_input_states(&other).await.unwrap().report.is_empty(), "wrong target cannot enqueue recipient work");
    }))).catch_unwind().await;
    fixture.close().await;
    match result {
        Ok(result) => result.expect("native refusals did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_native_targets_keep_audience_revocation_and_cancellation_fences() {
    let fixture = Box::pin(start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 6,
        Box::pin(async {
            let session = add(&fixture, ADDED).await;
            add(&fixture, "project-aborted").await;
            ready(&fixture, &session).await;
            let first = original(&fixture, "native-fence-basis").await;
            let request = selection(&fixture, "native-fence-basis").await;
            fixture
                .source
                .prepare_original_native_operations(first.clone(), request.clone())
                .await
                .unwrap();
            run(
                &fixture,
                &session,
                first.clone(),
                "peers",
                serde_json::json!({}),
                Some(true),
            )
            .await;
            let association = fixture
                .source
                .binding_evidence(&first.original)
                .await
                .unwrap()
                .unwrap();
            let directory = meerkat_core::ToolOperationTarget::PeerDirectoryRead;
            fixture
                .source
                .native_operation_values(&association, &directory)
                .unwrap();
            let mut wrong = association.candidate().clone();
            wrong.target.audience = AudienceRef::Principal {
                principal: principal("foreign-audience"),
            };
            let wrong =
                meerkat_authorization_contracts::work_association::InputAuthorityAssociation::new(
                    wrong,
                )
                .unwrap();
            assert!(matches!(
                fixture.source.native_operation_values(&wrong, &directory),
                Err(PrivateOriginalError::Denied)
            ));
            let lineage = association
                .candidate()
                .authority_basis
                .lineage_for(Some(&GrantTarget::PeerDirectory))
                .unwrap();
            let mut mutation = fixture.machine.try_controller_grant_mutation().unwrap();
            let authority = fixture
                .lifetime
                .selected_policy()
                .native_host
                .as_ref()
                .unwrap()
                .remote_operator
                .clone()
                .unwrap();
            assert_eq!(lineage[0].root_authority, authority);
            fixture
                .source
                .owner
                .grants()
                .revoke(&authority, &lineage[0], &mut mutation)
                .unwrap();
            drop(mutation);
            let grants = fixture.source.owner.grants();
            let claims = association.candidate();
            assert!(
                matches!(
                    grants.resolve_lineage(
                        lineage,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref()
                    ),
                    Err(meerkat_authorization::grants::GrantRefusal::Denied)
                ),
                "the actual revoked lineage must be unusable before the next submission"
            );
            let model_requests = fixture.provider.0.load(Ordering::SeqCst);
            let next = original(&fixture, "native-revoked-request").await;
            fixture
                .source
                .prepare_original_native_operations(next.clone(), request)
                .await
                .unwrap();
            let next_row = fixture
                .source
                .store
                .get(&fixture.source.key(&next.original).unwrap())
                .await
                .unwrap()
                .unwrap();
            let next_record = fixture
                .source
                .decode_record(&next_row, &next.original)
                .unwrap();
            // Observe this exact original's deterministic directory child. A valid
            // earlier target may issue its own child before directory refusal.
            let directory_child = EvidenceId::new(format!(
                "native-original-target/{}",
                digest(
                    &canonical_json(&(
                        "toolkit.native-original-target.v1",
                        &next_record.source,
                        &next_record.event,
                        &lineage[0].grant_id,
                        &GrantTarget::PeerDirectory,
                    ))
                    .unwrap()
                )
            ))
            .unwrap();
            assert!(grants.issued_reference(&directory_child).unwrap().is_none());
            // Binder refusal is an unavailable native-outcome projection, not an
            // outer Err or native acceptance. Inspect the owners below as well.
            match fixture
                .source
                .submit_authenticated_original(next.clone())
                .await
            {
                Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable) => {}
                Ok(PrivateOriginalSubmission::Admitted { .. }) => {
                    panic!("revoked-root submission returned Admitted")
                }
                Ok(PrivateOriginalSubmission::AdmissionUnresolved) => {
                    panic!("revoked-root submission returned AdmissionUnresolved")
                }
                Err(_) => panic!("revoked-root submission returned unexpected Err"),
            }
            assert!(
                grants.issued_reference(&directory_child).unwrap().is_none(),
                "the revoked directory root cannot issue this original's child"
            );
            for reference in lineage {
                assert_eq!(
                    grants
                        .issued_reference(&reference.grant_id)
                        .unwrap()
                        .as_ref(),
                    Some(reference),
                    "revocation must retain the exact issued references"
                );
            }
            assert!(
                matches!(
                    grants.resolve_lineage(
                        lineage,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref()
                    ),
                    Err(meerkat_authorization::grants::GrantRefusal::Denied)
                ),
                "the revoked root must not become usable after another original selects it"
            );
            assert!(
                fixture
                    .source
                    .binding_evidence(&next.original)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                fixture
                    .machine
                    .session_input_states(&session)
                    .await
                    .unwrap()
                    .report
                    .len(),
                1
            );
            let cancelled = original(&fixture, "native-cancelled-request").await;
            let cancellation_request = selection(&fixture, "native-cancelled-request").await;
            fixture
                .source
                .prepare_original_native_operations(cancelled.clone(), cancellation_request.clone())
                .await
                .unwrap();
            fixture
                .source
                .record_operation_cancelled(&cancelled.original)
                .await
                .unwrap();
            assert!(matches!(
                fixture
                    .source
                    .submit_authenticated_original(cancelled.clone())
                    .await,
                Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
            ));
            assert!(
                fixture
                    .source
                    .binding_evidence(&cancelled.original)
                    .await
                    .unwrap()
                    .is_none()
            );
            for selected in cancellation_request.targets {
                assert!(
                    fixture
                        .source
                        .owner
                        .grants()
                        .issued_reference(&selected.issuance_id)
                        .unwrap()
                        .is_none()
                );
            }
            let pending = original(&fixture, "native-retired-recipient").await;
            fixture
                .source
                .prepare_original_native_operations(
                    pending.clone(),
                    selection(&fixture, "native-retired-recipient").await,
                )
                .await
                .unwrap();
            let recipient = fixture
                .policy
                .member_class_admission(&instance("project-aborted").member.member)
                .unwrap()
                .unwrap();
            fixture
                .policy
                .retire_member_class(&recipient.id, &recipient.instance.member.member)
                .await
                .unwrap();
            assert!(matches!(
                fixture
                    .source
                    .submit_authenticated_original(pending.clone())
                    .await,
                Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
            ));
            assert!(
                fixture
                    .source
                    .binding_evidence(&pending.original)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                matches!(
                    fixture
                        .source
                        .native_operation_values(&association, &directory),
                    Err(PrivateOriginalError::Denied)
                ),
                "a retained target admission cannot be replaced by the same visible route"
            );
            assert_eq!(
                fixture
                    .machine
                    .session_input_states(&session)
                    .await
                    .unwrap()
                    .report
                    .len(),
                1
            );
            assert_eq!(
                fixture.provider.0.load(Ordering::SeqCst),
                model_requests,
                "refused submissions cannot dispatch another model turn"
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match result {
        Ok(result) => result.expect("native fences did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_issuance_retains_source_and_member_guards_after_observer_cancellation() {
    let captured = Arc::new(std::sync::OnceLock::new());
    let capture = captured.clone();
    let fixture = Box::pin(start_with_policy_store(Some(Box::new(move |inner| {
        let store = Arc::new(super::workflow::HeldSourceRead::new(inner));
        assert!(capture.set(store.clone()).is_ok());
        store
    }))))
    .await;
    let store = captured.get().unwrap().clone();
    let mut observer = None;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 6,
        Box::pin(async {
            let session = add(&fixture, ADDED).await;
            ready(&fixture, &session).await;
            let submission = original(&fixture, "native-lost-issuer-observer").await;
            let mut request = selection(&fixture, "native-lost-issuer-observer").await;
            request
                .targets
                .retain(|row| matches!(row.target, NativeOriginalTarget::PeerDirectory));
            fixture
                .source
                .prepare_original_native_operations(submission.clone(), request.clone())
                .await
                .unwrap();
            let key = fixture.source.key(&submission.original).unwrap();
            let before = fixture.source.store.get(&key).await.unwrap().unwrap();
            let record = fixture
                .source
                .decode_record(&before, &submission.original)
                .unwrap();
            let admission = fixture
                .policy
                .member_class_admission(&instance(ADDED).member.member)
                .unwrap()
                .unwrap();
            let policy_rows = fixture
                .source
                .store
                .list_records(&fixture.source.scope, None, 256)
                .await
                .unwrap()
                .into_iter()
                .filter(|row| row.data.schema == "toolkit.application-service-tool-policy")
                .collect::<Vec<_>>();
            let [policy_row] = policy_rows.as_slice() else {
                panic!("one actual member-policy owner required");
            };
            let root_id = request.targets[0].issuance_id.clone();
            assert_eq!(
                crate::private_originals::native_operations::selected_targets(&record),
                [GrantTarget::PeerDirectory]
            );
            // The existing deterministic child identity lets the real grant owner
            // prove both writes survived observer loss, before any input is bound.
            let child_id = EvidenceId::new(format!(
                "native-original-target/{}",
                digest(
                    &canonical_json(&(
                        "toolkit.native-original-target.v1",
                        &record.source,
                        &record.event,
                        &root_id,
                        &GrantTarget::PeerDirectory,
                    ))
                    .unwrap()
                )
            ))
            .unwrap();
            for id in [&root_id, &child_id] {
                assert!(
                    fixture
                        .source
                        .owner
                        .grants()
                        .issued_reference(id)
                        .unwrap()
                        .is_none()
                );
            }
            // The first policy read in this issuer is inside with_current_member,
            // after the retained blocking task holds both real owner guards.
            *store.held_key.lock().unwrap() = Some(policy_row.key.clone());
            let source = fixture.source.clone();
            let reference = submission.original.clone();
            let issue_session = session.clone();
            observer = Some(tokio::spawn(async move {
                source
                    .native_operations_basis(
                        &reference,
                        &record,
                        &issue_session,
                        WorkAuthorityBasis::TargetedGrantLineages {
                            base: None,
                            targets: vec![],
                        },
                    )
                    .await
            }));
            store.entered.notified().await;
            let task = observer.take().unwrap();
            task.abort();
            assert!(
                matches!(task.await, Err(error) if error.is_cancelled()),
                "the async observer really was cancelled"
            );

            assert!(
                matches!(
                    fixture.source.owner.try_source_writer(),
                    Err(crate::original_ingress::OriginalWriterError::Unavailable)
                ),
                "dropping the observer must not release the entered issuer's source writer"
            );
            assert!(
                matches!(
                    fixture
                        .source
                        .record_operation_cancelled(&submission.original)
                        .await,
                    Err(PrivateOriginalError::Unavailable)
                ),
                "source cancellation cannot overtake entered issuance"
            );
            assert_eq!(
                fixture.source.store.get(&key).await.unwrap(),
                Some(before.clone())
            );
            let writer_wait = fixture.source.owner.observe_source_writer_wait_for_test();
            let barrier = async {
                drop(fixture.source.owner.source_writer().await);
            };
            tokio::pin!(barrier);
            assert!(futures::poll!(&mut barrier).is_pending());
            writer_wait.await.unwrap();
            let retirement = fixture
                .policy
                .retire_member_class(&admission.id, &admission.instance.member.member);
            tokio::pin!(retirement);
            assert!(
                futures::poll!(&mut retirement).is_pending(),
                "actual member retirement waits for entered issuance"
            );
            assert_eq!(
                fixture
                    .policy
                    .member_class_admission(&admission.instance.member.member)
                    .unwrap(),
                Some(admission.clone())
            );
            assert_eq!(
                fixture.source.store.get(&policy_row.key).await.unwrap(),
                Some(policy_row.clone())
            );
            for id in [&root_id, &child_id] {
                assert!(
                    fixture
                        .source
                        .owner
                        .grants()
                        .issued_reference(id)
                        .unwrap()
                        .is_none()
                );
            }

            store.release.add_permits(1);
            barrier.await; // The same source writer joins the retained issuer's entire effect section.
            let root = fixture
                .source
                .owner
                .grants()
                .issued_reference(&root_id)
                .unwrap()
                .expect("entered root was issued");
            let child = fixture
                .source
                .owner
                .grants()
                .issued_reference(&child_id)
                .unwrap()
                .expect("entered child was issued");
            assert_eq!(
                root.root_authority,
                fixture
                    .lifetime
                    .selected_policy()
                    .native_host
                    .as_ref()
                    .unwrap()
                    .remote_operator
                    .clone()
                    .unwrap()
            );
            fixture
                .source
                .owner
                .grants()
                .resolve_lineage(
                    &[root.clone(), child.clone()],
                    &fixture.route(ADDED).logical_executor,
                    fixture.route(ADDED).represented_subject.as_ref(),
                )
                .unwrap()
                .check_current()
                .unwrap();
            fixture
                .source
                .record_operation_cancelled(&submission.original)
                .await
                .unwrap();
            let after = fixture.source.store.get(&key).await.unwrap().unwrap();
            let cancelled = fixture
                .source
                .decode_record(&after, &submission.original)
                .unwrap();
            assert!(cancelled.operation_cancelled && cancelled.final_association.is_none());
            assert!(
                cancelled.native_operations
                    == fixture
                        .source
                        .decode_record(&before, &submission.original)
                        .unwrap()
                        .native_operations,
                "cancellation retains the exact selected request and member identities"
            );
            // The current route is needed for authorized reads. After retirement,
            // only canonical history may expose this already-acknowledged row.
            retirement.await.unwrap();
            assert!(
                matches!(
                    fixture.source.decode_record(&after, &submission.original),
                    Err(PrivateOriginalError::Unavailable)
                ),
                "the retired caller must no longer authorize a current source read"
            );
            assert_eq!(
                fixture.source.store.get(&key).await.unwrap(),
                Some(after.clone()),
                "retirement must preserve the exact acknowledged cancellation row"
            );
            let historical = fixture.source.decode_stored(&after).unwrap();
            assert_eq!(
                canonical_json(&historical).unwrap(),
                canonical_json(&cancelled).unwrap(),
                "canonical history retains the outcome without restoring caller authority"
            );
            assert!(matches!(
                fixture
                    .policy
                    .member_class_admission(&admission.instance.member.member),
                Err(HostLifetimeError::Denied)
            ));
            assert_eq!(
                fixture
                    .source
                    .owner
                    .grants()
                    .issued_reference(&root_id)
                    .unwrap(),
                Some(root)
            );
            assert_eq!(
                fixture
                    .source
                    .owner
                    .grants()
                    .issued_reference(&child_id)
                    .unwrap(),
                Some(child)
            );
            assert!(
                fixture
                    .machine
                    .session_input_states(&session)
                    .await
                    .unwrap()
                    .report
                    .is_empty(),
                "issuance is not input submission"
            );
        }),
    ))
    .catch_unwind()
    .await;
    // Before the held read, the issuer may still be queued: release and join
    // its observer normally. After the intentional abort, the held writer is
    // proven and its barrier joins the otherwise unobserved effect section.
    store.release.add_permits(1);
    if let Some(task) = observer {
        let _ = task.await;
    }
    drop(fixture.source.owner.source_writer().await);
    fixture.close().await;
    match result {
        Ok(result) => result.expect("lost-observer issuance did not settle"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_add_retains_exact_standing_parents_before_activation_and_revoke_never_refreshes() {
    use crate::installed_host::{
        application_operations, dynamic_members::MemberClassAdmissionPhase,
    };
    use meerkat_authorization_contracts::constraints::{
        ActionRef, DomainRestriction, ExactRestriction,
    };
    let fixture = Box::pin(start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 5,
        Box::pin(async {
            let admitted = fixture.admit(ADDED).await;
            assert!(admitted.admission.standing_operations.is_none());
            let policy = fixture.policy.clone();
            let source = fixture.source.clone();
            let finalized = Arc::new(AtomicBool::new(false));
            let checked = finalized.clone();
            let started = fixture
                .gateway
                .add_member_class(
                    &fixture.native.identity_first_context().unwrap(),
                    admitted.clone(),
                    spec(ADDED, &class().profile),
                    fixture.route(ADDED),
                    fixture.intent(ADDED),
                    tokio::time::Instant::now() + WAIT,
                    move || async move {
                        let row = policy
                            .member_class_admission(&instance(ADDED).member.member)?
                            .ok_or(HostLifetimeError::Denied)?;
                        assert!(matches!(
                            row.phase,
                            MemberClassAdmissionPhase::Prepared { .. }
                        ));
                        let standing = row
                            .standing_operations
                            .as_ref()
                            .expect("new Add retains its intent");
                        let issued = standing
                            .issued
                            .as_ref()
                            .expect("both refs precede application finalization");
                        for root in [&issued.peer_message, &issued.recorder_write] {
                            assert_eq!(
                                source
                                    .owner
                                    .grants()
                                    .issued_reference(&root.grant_id)
                                    .unwrap()
                                    .as_ref(),
                                Some(root)
                            );
                        }
                        checked.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
                .unwrap();
            let outcome = match started {
                MemberClassAddStart::Settled(value) => value,
                MemberClassAddStart::Unsettled(value) => value.settled().await.unwrap(),
            };
            assert!(
                outcome.authorization.is_ok() && outcome.cleanup.is_ok(),
                "{outcome:?}"
            );
            assert!(matches!(
                outcome
                    .native
                    .unwrap()
                    .outcomes
                    .get(&AgentIdentity::parse(ADDED).unwrap()),
                Some(RestoreOutcome::Created { .. })
            ));
            assert!(finalized.load(Ordering::SeqCst));
            let row = fixture
                .policy
                .member_class_admission(&instance(ADDED).member.member)
                .unwrap()
                .unwrap();
            let before = serde_json::to_value(&row).unwrap();
            let standing = row.standing_operations.as_ref().unwrap();
            let issued = standing.issued.as_ref().unwrap();
            let route = row.route.as_ref().unwrap();
            let context = fixture.source.callback_grant_context().unwrap();
            let grants = &context.grants;
            let mut expected_peer = application_operations::configured_catalog_parent(
                &context.application,
                &route.logical_executor,
                &row.instance.member.audience,
            );
            expected_peer.actions = ExactRestriction::exact([ActionRef {
                feature: "toolkit".into(),
                action: "peer-message".into(),
            }]);
            expected_peer.resource_domains =
                DomainRestriction::owned_by([context.application.clone()]);
            let expected_memory = application_operations::parent_restrictions(
                application_operations::recorder_operation(
                    &context.application,
                    &route.logical_executor,
                    &row.instance.member.audience,
                    &standing.recorder_scope,
                    meerkat_core::MemoryOperation::Write,
                )
                .unwrap(),
            );
            for (root, bounds) in [
                (&issued.peer_message, &expected_peer),
                (&issued.recorder_write, &expected_memory),
            ] {
                assert_eq!(root.root_authority, context.operator);
                let resolved = grants
                    .resolve_lineage(
                        std::slice::from_ref(root),
                        &context.application,
                        route.represented_subject.as_ref(),
                    )
                    .unwrap();
                assert_eq!(resolved.restrictions(), bounds);
                resolved.check_current().unwrap();
            }
            assert_eq!(
                standing.recorder_scope,
                meerkat_mobkit::memory::records::MemoryScope::Identity {
                    realm: fixture
                        .lifetime
                        .selected_policy()
                        .native_host
                        .as_ref()
                        .unwrap()
                        .realm
                        .to_string(),
                    identity: ADDED.into(),
                }
                .operation_scope_ref()
            );
            let replay = fixture.admit(ADDED).await;
            assert!(replay.add_token.is_none());
            assert_eq!(replay.claim_add(), Err(HostLifetimeError::Denied));
            let same = admitted
                .owner
                .with_current_member(
                    admitted.admission_id(),
                    admitted.class(),
                    admitted.instance(),
                    || admitted.owner.issue_standing_parents(&row),
                )
                .await
                .unwrap();
            assert_eq!(&same, issued);
            let mut mutation = fixture.machine.try_controller_grant_mutation().unwrap();
            grants
                .revoke(&context.operator, &issued.peer_message, &mut mutation)
                .unwrap();
            drop(mutation);
            assert_eq!(
                admitted
                    .owner
                    .with_current_member(
                        admitted.admission_id(),
                        admitted.class(),
                        admitted.instance(),
                        || admitted.owner.issue_standing_parents(&row)
                    )
                    .await,
                Err(HostLifetimeError::Denied)
            );
            assert_eq!(
                serde_json::to_value(
                    fixture
                        .policy
                        .member_class_admission(&instance(ADDED).member.member)
                        .unwrap()
                        .unwrap()
                )
                .unwrap(),
                before
            );
            let session = fixture
                .native
                .identity_runtime()
                .unwrap()
                .status(&AgentIdentity::parse(ADDED).unwrap())
                .await
                .unwrap()
                .session_id
                .unwrap();
            assert!(native_input_states(&fixture, &session).await.is_empty());
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
            admitted.retire().await.unwrap();
            for root in [&issued.peer_message, &issued.recorder_write] {
                assert_eq!(
                    grants.issued_reference(&root.grant_id).unwrap().as_ref(),
                    Some(root)
                );
                assert!(matches!(
                    grants.resolve_lineage(
                        std::slice::from_ref(root),
                        &context.application,
                        route.represented_subject.as_ref()
                    ),
                    Err(meerkat_authorization::grants::GrantRefusal::Denied)
                ));
            }
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("standing Add control timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partial_standing_parent_issue_retains_ids_and_retires_without_activation() {
    use crate::installed_host::{
        application_operations, dynamic_members::MemberClassAdmissionPhase,
    };
    let fixture = Box::pin(start()).await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 5,
        Box::pin(async {
            let admitted = fixture.admit(ADDED).await;
            let bundle = admitted.owner.callback_bundle(&admitted.admission).unwrap();
            let peer_id = EvidenceId::new(format!("{bundle}/peer-message")).unwrap();
            let recorder_id = EvidenceId::new(format!("{bundle}/recorder-write")).unwrap();
            let context = fixture.source.callback_grant_context().unwrap();
            let route = fixture.route(ADDED);
            // The same native owner already has this exact ID with incompatible
            // bounds. Reconciliation must fail after the first parent is issued.
            let wrong = application_operations::configured_catalog_parent(
                &context.application,
                &route.logical_executor,
                &instance(ADDED).member.audience,
            );
            let existing = context
                .grants
                .issue_root(
                    &context.operator,
                    recorder_id.clone(),
                    context.application.clone(),
                    route.represented_subject.clone(),
                    wrong,
                )
                .unwrap();
            assert!(context.grants.issued_reference(&peer_id).unwrap().is_none());
            let finalized = Arc::new(AtomicBool::new(false));
            let flag = finalized.clone();
            let started = fixture
                .gateway
                .add_member_class(
                    &fixture.native.identity_first_context().unwrap(),
                    admitted.clone(),
                    spec(ADDED, &class().profile),
                    route.clone(),
                    fixture.intent(ADDED),
                    tokio::time::Instant::now() + WAIT,
                    move || async move {
                        flag.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
                .unwrap();
            let outcome = match started {
                MemberClassAddStart::Settled(value) => value,
                MemberClassAddStart::Unsettled(value) => value.settled().await.unwrap(),
            };
            assert_eq!(outcome.authorization, Err(HostLifetimeError::Denied));
            assert!(outcome.cleanup.is_ok(), "{outcome:?}");
            assert!(!finalized.load(Ordering::SeqCst));
            assert!(!outcome.native.as_ref().is_ok_and(|value| matches!(
                value.outcomes.get(&AgentIdentity::parse(ADDED).unwrap()),
                Some(RestoreOutcome::Created { .. })
            )));
            let observed = fixture
                .policy
                .observe_member_class(&instance(ADDED).member.member)
                .await
                .unwrap();
            assert_eq!(observed.authorization, Err(HostLifetimeError::Denied));
            let row = observed.admission;
            assert_eq!(row.phase, MemberClassAdmissionPhase::Retired);
            let intent = row
                .standing_operations
                .as_ref()
                .expect("partial issue keeps its canonical intent");
            assert_eq!(intent.peer_message_parent, peer_id);
            assert_eq!(intent.recorder_write_parent, recorder_id);
            assert!(
                intent.issued.is_none(),
                "an incomplete pair cannot be committed as complete"
            );
            let first = context
                .grants
                .issued_reference(&peer_id)
                .unwrap()
                .expect("first issue actually happened");
            assert_eq!(
                context.grants.issued_reference(&recorder_id).unwrap(),
                Some(existing.clone())
            );
            for root in [&first, &existing] {
                assert!(matches!(
                    context.grants.resolve_lineage(
                        std::slice::from_ref(root),
                        &context.application,
                        route.represented_subject.as_ref()
                    ),
                    Err(meerkat_authorization::grants::GrantRefusal::Denied)
                ));
            }
            assert!(fixture.source.member_route(&route.id).is_err());
            assert_eq!(
                admitted
                    .owner
                    .with_current_member(
                        admitted.admission_id(),
                        admitted.class(),
                        admitted.instance(),
                        || admitted.owner.issue_standing_parents(&row)
                    )
                    .await,
                Err(HostLifetimeError::Denied)
            );
            assert_eq!(
                context.grants.issued_reference(&peer_id).unwrap(),
                Some(first)
            );
            assert_eq!(
                context.grants.issued_reference(&recorder_id).unwrap(),
                Some(existing)
            );
            let session =
                meerkat_core::SessionId::parse(row.destination.as_ref().unwrap().context.as_str())
                    .unwrap();
            assert!(native_input_states(&fixture, &session).await.is_empty());
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 0);
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("partial standing Add control timed out"),
    }
}
