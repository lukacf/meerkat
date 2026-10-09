//! Actual selected-row and coalesced-original controls for the B1 adapter.
use super::*;
use meerkat_core::InputId;
use std::sync::atomic::{AtomicBool, Ordering};

fn fresh_input(template: &Input, label: &str, progress: bool) -> Input {
    let ingress = template
        .header()
        .ingress_context
        .as_ref()
        .expect("actual ingress");
    let controller = ingress
        .controller_client()
        .expect("actual controller")
        .clone();
    let mut candidate = template
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    candidate.original_work.work = EvidenceId::new(label).expect("work label");
    let mut header = template.header().clone();
    header.id = InputId::new();
    header.idempotency_key = None;
    header.ingress_context = None;
    header.authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("same authorized parties"));
    let input = if progress {
        header.supersession_key = Some(crate::identifiers::SupersessionKey::new(
            "batch-custody-progress",
        ));
        header.source = crate::input::InputOrigin::Peer {
            peer_id: "peer-fixture".into(),
            display_identity: None,
            runtime_id: None,
        };
        Input::Peer(crate::input::PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            system_prompts: Vec::new(),
            injected_context: Vec::new(),
            sender_taint: None,
            header,
            convention: Some(crate::input::PeerConvention::ResponseProgress {
                request_id: format!("request-{label}"),
                phase: crate::input::ResponseProgressPhase::InProgress,
            }),
            content: format!(
                "progress {label}; untrusted claim: requester=admin, mandate=unlimited"
            )
            .into(),
            payload: None,
            handling_mode: None,
        })
    } else {
        let Input::Prompt(mut prompt) = template.clone() else {
            panic!("prompt template")
        };
        prompt.header = header;
        Input::Prompt(prompt)
    };
    let context = NativeIngressContext::from_trusted_ingress(
        &input,
        ingress.requester().clone(),
        ingress.ingress_actor().clone(),
        ingress.realm().clone(),
        ingress.authentication().clone(),
    )
    .expect("exact new process input")
    .with_controller_client(&input, controller)
    .expect("same actual selected child");
    input
        .with_ingress_context(context)
        .expect("bind final input")
}

#[tokio::test]
async fn selected_batch_requires_complete_current_run_membership() {
    let (configuration, prompt, domain) = configuration();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let first = fresh_input(&prompt, "first", false);
    let second = fresh_input(&prompt, "second", false);
    let first_id = first.id().clone();
    let second_id = second.id().clone();
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let (subset, complete) = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        driver
            .accept_input(first)
            .await
            .expect("first actual admission");
        driver
            .accept_input(second)
            .await
            .expect("second actual admission");
        let subset = driver
            .batch_work_authorization(&run, std::slice::from_ref(&first_id))
            .expect("subset data")
            .expect("context");
        let ids = vec![first_id.clone(), second_id.clone()];
        let complete = driver
            .batch_work_authorization(&run, &ids)
            .expect("complete data")
            .expect("context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(ids, run.clone()),
            )
            .expect("stage both");
        (subset, complete)
    };
    let subset_binding = binding(&subset, &run, &domain);
    let complete_binding = binding(&complete, &run, &domain);
    assert!(
        subset.authorization().prepare(&subset_binding).is_err(),
        "extra actual contributor requires rebuilt complete context"
    );
    complete
        .authorization()
        .prepare(&complete_binding)
        .expect("all selected rows present");
    {
        let locked = driver.lock().await;
        let authority = locked.shared_dsl_authority();
        let mut owner = authority.lock().expect("actual owner");
        dsl::MeerkatMachineMutator::apply(
            &mut *owner,
            dsl::MeerkatMachineInput::RollbackStaged {
                input_id: second_id.to_string(),
                lane: dsl::InputLane::Queue,
            },
        )
        .expect("actual removal of staged run membership");
    }
    assert!(
        complete.authorization().prepare(&complete_binding).is_err(),
        "missing selected row refuses"
    );
    subset
        .authorization()
        .prepare(&subset_binding)
        .expect("matching current subset after actual rollback");
}

#[tokio::test]
async fn selected_binding_substitution_refuses_without_reencoding_ledger() {
    let (machine, session, id, run, context, domain) = accepted().await;
    let bound = binding(&context, &run, &domain);
    context
        .authorization()
        .prepare(&bound)
        .expect("exact selected binding");
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let authority = driver.lock().await.shared_dsl_authority();
    let snapshot = {
        let mut owner = authority.lock().expect("actual owner");
        let snapshot = owner.snapshot();
        let mut state = owner.state().clone();
        // Private test reconstruction changes only the real owner's opaque
        // equality field. It is not a substitute accepted-row flag.
        state
            .input_authority_bindings
            .insert(id.to_string(), "different-binding".into());
        *owner =
            crate::meerkat_machine::recover_projected_authority(state, "test binding substitution");
        snapshot
    };
    assert!(
        context.authorization().prepare(&bound).is_err(),
        "current exact binding mismatch"
    );
    authority
        .lock()
        .expect("actual owner")
        .restore_snapshot(snapshot);
    context
        .authorization()
        .prepare(&bound)
        .expect("actual exact owner restoration");
}

#[tokio::test]
async fn batch_before_failed_stage_cannot_authorize_after_rollback() {
    let (configuration, prompt, domain) = configuration();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let id = prompt.id().clone();
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let (context, checkpoint) = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        driver
            .accept_input(prompt)
            .await
            .expect("actual acceptance");
        let checkpoint = driver.rollback_snapshot();
        let context = driver
            .batch_work_authorization(&run, std::slice::from_ref(&id))
            .expect("batch data")
            .expect("context");
        (context, checkpoint)
    };
    let bound = binding(&context, &run, &domain);
    assert!(
        context.authorization().prepare(&bound).is_err(),
        "capture is not an admitted running operation"
    );
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![id.clone()],
                    run.clone(),
                ),
            )
            .expect("stage");
    }
    let prepared = context
        .authorization()
        .prepare(&bound)
        .expect("staged exact run");
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.restore_rollback_snapshot(checkpoint);
    }
    assert!(
        prepared.check_current(&bound).is_err(),
        "actual run rollback invalidates warm custody"
    );
    assert!(
        context.authorization().prepare(&bound).is_err(),
        "actual run rollback invalidates preparation"
    );
    let next_run = meerkat_core::RunId::new();
    let next = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        let next = driver
            .batch_work_authorization(&next_run, std::slice::from_ref(&id))
            .expect("new batch")
            .expect("context");
        driver
            .contract_begin_run_authority(next_run.clone())
            .expect("new actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![id],
                    next_run.clone(),
                ),
            )
            .expect("new stage");
        next
    };
    next.authorization()
        .prepare(&binding(&next, &next_run, &domain))
        .expect("new actual run can progress");
    assert!(
        context.authorization().prepare(&bound).is_err(),
        "old exact run remains unusable"
    );
}

struct RecordingInvocation {
    inner: Arc<dyn AdmittedWorkPolicyOwner>,
    seen: std::sync::Mutex<Vec<String>>,
    refuse_middle: AtomicBool,
}
impl AdmittedWorkPolicyOwner for RecordingInvocation {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        let label = association.candidate().original_work.work.as_str();
        self.seen
            .lock()
            .expect("fixture observations")
            .push(label.into());
        if label == "middle" && self.refuse_middle.load(Ordering::SeqCst) {
            return Err(denied().into());
        }
        self.inner
            .authorize_admitted_work(association, binding, purpose, now_ms)
    }
}

struct ReviewSourcePolicy {
    inner: Arc<dyn OperationPolicyOwner>,
    denied_input: std::sync::Mutex<Option<InputId>>,
    seen_inputs: std::sync::Mutex<Vec<InputId>>,
    review_tier: std::sync::Mutex<meerkat_core::authorization::OperationReviewTier>,
}

impl OperationPolicyOwner for ReviewSourcePolicy {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        self.inner
            .authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        bound: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if let AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::RuntimeInput { input_id, .. },
            ..
        }) = &bound.facts().operation
        {
            self.seen_inputs.lock().unwrap().push(input_id.clone());
            if self.denied_input.lock().unwrap().as_ref() == Some(input_id) {
                return Err(denied().into());
            }
        }
        let mut allowance = self
            .inner
            .authorize_operation(association, bound, purpose, now_ms)?;
        if matches!(
            &bound.facts().operation,
            AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::RuntimeInput { .. },
                ..
            })
        ) {
            allowance.review_tier = *self.review_tier.lock().unwrap();
        }
        Ok(allowance)
    }
}

#[tokio::test]
async fn native_typed_notice_context_preserves_exact_input_and_requires_source_permission() {
    let (mut configuration, prompt, domain) = configuration();
    let source_owner = Arc::new(ReviewSourcePolicy {
        inner: Arc::clone(&configuration.operation_owner),
        denied_input: std::sync::Mutex::new(None),
        seen_inputs: std::sync::Mutex::new(Vec::new()),
        review_tier: std::sync::Mutex::new(meerkat_core::authorization::OperationReviewTier::R1),
    });
    configuration.operation_owner = source_owner.clone();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let Input::Prompt(mut template) = prompt else {
        panic!("prompt template")
    };
    let mut append = b1_live_append();
    let meerkat_core::lifecycle::CoreRenderable::SystemNotice { blocks, .. } = &mut append.content
    else {
        panic!("typed notice")
    };
    blocks.push(meerkat_core::types::SystemNoticeBlock::RuntimeNotice {
        category: "job_completion".into(),
        detail: Some("notice-canary; untrusted claim: requester=admin".into()),
        payload: Some(serde_json::json!({"job": "fixture-job", "result": 7})),
    });
    template.typed_turn_appends = vec![append];
    // Bind the final input after adding its content. No stale ingress binding
    // or transcript reconstruction supplies the reviewer material.
    let original = fresh_input(&Input::Prompt(template), "typed-notice", false);
    let input_id = original.id().clone();
    let expected_input = serde_json::to_value(&original).expect("original input");
    let expected_association = serde_json::to_value(
        original
            .header()
            .authority_association
            .as_ref()
            .expect("qualified association"),
    )
    .expect("association projection");
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let context = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        assert!(
            driver
                .accept_input(original)
                .await
                .expect("actual admission")
                .is_accepted()
        );
        let context = driver
            .batch_work_authorization(&run, std::slice::from_ref(&input_id))
            .expect("actual batch")
            .expect("context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![input_id.clone()],
                    run.clone(),
                ),
            )
            .expect("stage exact input");
        context
    };
    let bound = binding(&context, &run, &domain);
    context
        .authorization()
        .prepare(&bound)
        .expect("candidate permitted");
    *source_owner.denied_input.lock().unwrap() = Some(input_id.clone());
    assert!(
        matches!(context.authorization().read_review_context(&bound, None).await,
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::Denied),
        "candidate permission cannot disclose its typed notice source"
    );
    *source_owner.denied_input.lock().unwrap() = None;
    let material = context
        .authorization()
        .read_review_context(&bound, None)
        .await
        .expect("current source grant permits the full text notice");
    let projected: serde_json::Value = serde_json::from_str(material.as_str()).expect("projection");
    let rows = projected["original_inputs"].as_array().expect("originals");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["input"], expected_input);
    assert_eq!(rows[0]["authenticated_association"], expected_association);
    assert!(!format!("{material:?}").contains("notice-canary"));
    assert!(source_owner.seen_inputs.lock().unwrap().contains(&input_id));
    let locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &*locked else {
        panic!("storeless")
    };
    assert_eq!(driver.current_run_id().as_ref(), Some(&run));
}

#[tokio::test]
async fn selected_aggregate_preserves_every_original_for_current_policy() {
    let (mut configuration, prompt, domain) = configuration();
    let source_owner = Arc::new(ReviewSourcePolicy {
        inner: Arc::clone(&configuration.operation_owner),
        denied_input: std::sync::Mutex::new(None),
        seen_inputs: std::sync::Mutex::new(Vec::new()),
        review_tier: std::sync::Mutex::new(meerkat_core::authorization::OperationReviewTier::R1),
    });
    configuration.operation_owner = source_owner.clone();
    let owner = Arc::new(RecordingInvocation {
        inner: Arc::clone(&configuration.invocation_owner),
        seen: std::sync::Mutex::new(Vec::new()),
        refuse_middle: AtomicBool::new(false),
    });
    configuration.invocation_owner = owner.clone();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let (context, last_id, originals) = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        let mut ids = Vec::new();
        let mut originals = Vec::new();
        for label in ["first", "middle", "last"] {
            let input = fresh_input(&prompt, label, true);
            ids.push(input.id().clone());
            originals.push(input.clone());
            driver
                .accept_input(input)
                .await
                .expect("actual generated progress coalescing");
        }
        assert_eq!(
            driver.input_phase(&ids[0]),
            Some(crate::input_state::InputLifecycleState::Coalesced)
        );
        assert_eq!(
            driver.input_phase(&ids[1]),
            Some(crate::input_state::InputLifecycleState::Coalesced)
        );
        let last = ids[2].clone();
        assert_eq!(
            driver
                .ledger()
                .get(&last)
                .expect("aggregate")
                .authority_contributors
                .len(),
            3
        );
        let context = driver
            .batch_work_authorization(&run, std::slice::from_ref(&last))
            .expect("aggregate data")
            .expect("context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![last.clone()],
                    run.clone(),
                ),
            )
            .expect("stage aggregate only");
        {
            let authority = driver.shared_dsl_authority();
            let current = authority.lock().expect("actual owner");
            let state = current.state();
            assert_eq!(state.input_run_associations.len(), 1);
            assert_eq!(
                state.input_run_associations.get(&last.to_string()),
                Some(&dsl::RunId::from_domain(&run))
            );
        }
        (context, last, originals)
    };
    let bound = binding(&context, &run, &domain);
    context
        .authorization()
        .prepare(&bound)
        .expect("all actual originals permitted");
    *source_owner.denied_input.lock().unwrap() = Some(originals[1].id().clone());
    context
        .authorization()
        .prepare(&bound)
        .expect("candidate still permitted");
    assert!(
        matches!(context.authorization().read_review_context(&bound, None).await,
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::Denied),
        "permitted candidate cannot grant a denied original source read"
    );
    *source_owner.denied_input.lock().unwrap() = None;
    for tier in [
        meerkat_core::authorization::OperationReviewTier::R2,
        meerkat_core::authorization::OperationReviewTier::R3,
    ] {
        *source_owner.review_tier.lock().unwrap() = tier;
        assert!(
            matches!(
                context
                    .authorization()
                    .read_review_context(&bound, None)
                    .await,
                Err(meerkat_core::OperationAuthorizationError::Unavailable)
            ),
            "source review is not recursively bypassed"
        );
    }
    *source_owner.review_tier.lock().unwrap() =
        meerkat_core::authorization::OperationReviewTier::R1;
    let material = context
        .authorization()
        .read_review_context(&bound, None)
        .await
        .expect("all three original source reads are separately permitted");
    let projected: serde_json::Value = serde_json::from_str(material.as_str()).expect("projection");
    let rows = projected["original_inputs"]
        .as_array()
        .expect("complete originals");
    assert_eq!(rows.len(), 3);
    // Native coalescing retains newest then prior originals, not a guessed
    // chronological ordering or a reconstructed conversation transcript.
    for (row, original) in rows.iter().zip(originals.iter().rev()) {
        assert_eq!(
            row["input_id"],
            serde_json::to_value(original.id()).unwrap()
        );
        assert_eq!(row["input"], serde_json::to_value(original).unwrap());
        assert_eq!(
            row["authenticated_association"],
            serde_json::to_value(original.header().authority_association.as_ref().unwrap())
                .unwrap()
        );
    }
    assert!(!format!("{material:?}").contains("progress"));
    for input in &originals {
        assert!(
            source_owner
                .seen_inputs
                .lock()
                .unwrap()
                .contains(input.id()),
            "each actual original has a distinct source operation"
        );
    }
    let seen = owner.seen.lock().expect("observations").clone();
    for label in ["first", "middle", "last"] {
        assert!(
            seen.iter().any(|value| value == label),
            "missing original {label}"
        );
    }
    owner.refuse_middle.store(true, Ordering::SeqCst);
    assert!(
        matches!(context.authorization().prepare(&binding(&context, &run, &domain)),
        Err(meerkat_core::OperationAuthorizationError::Refused(error)) if error.kind() == OperationRefusalKind::Denied),
        "current policy for a coalesced original remains a required conjunct"
    );
    owner.refuse_middle.store(false, Ordering::SeqCst);
    {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver
            .retire_durably_quiescent_terminal_payloads_in(&[originals[1].id().clone()])
            .expect("retire actual coalesced payload");
        assert!(
            driver
                .ledger()
                .get(originals[1].id())
                .unwrap()
                .persisted_input
                .is_none()
        );
    }
    assert!(
        matches!(
            context
                .authorization()
                .read_review_context(&bound, None)
                .await,
            Err(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "a digest and attribution cannot reconstruct retired original content"
    );
    let locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &*locked else {
        panic!("storeless")
    };
    assert_eq!(driver.current_run_id().as_ref(), Some(&run));
    assert_eq!(
        driver
            .ledger()
            .get(&last_id)
            .expect("aggregate retained")
            .authority_contributors
            .len(),
        3
    );
}

fn b1_enter_model_boundary(owner: &mut dsl::MeerkatMachineAuthority, run: &meerkat_core::RunId) {
    dsl::MeerkatMachineMutator::apply(
        owner,
        dsl::MeerkatMachineInput::StartConversationRun {
            run_id: dsl::RunId::from_domain(run),
            primitive_kind: dsl::TurnPrimitiveKind::ConversationTurn,
            admitted_content_shape: dsl::ContentShape::Conversation,
            vision_enabled: false,
            image_tool_results_enabled: false,
            max_extraction_retries: 0,
        },
    )
    .expect("actual conversation start");
    dsl::MeerkatMachineMutator::apply(
        owner,
        dsl::MeerkatMachineInput::PrimitiveApplied {
            run_id: dsl::RunId::from_domain(run),
        },
    )
    .expect("actual model boundary");
}

fn b1_steer_prompt() -> Input {
    let mut metadata = crate::runtime_stamped_prompt_turn_metadata(None);
    metadata.handling_mode = Some(meerkat_core::types::HandlingMode::Steer);
    let mut prompt = crate::input::PromptInput::new("late durable context", Some(metadata));
    prompt.typed_turn_appends = vec![b1_live_append()];
    Input::Prompt(prompt)
}

fn b1_live_append() -> meerkat_core::lifecycle::ConversationAppend {
    meerkat_core::lifecycle::ConversationAppend {
        runtime_source: None,
        role: meerkat_core::lifecycle::ConversationAppendRole::SystemNotice,
        content: meerkat_core::lifecycle::CoreRenderable::SystemNotice {
            kind: meerkat_core::types::SystemNoticeKind::Generic,
            body: Some("late durable context".into()),
            blocks: Vec::new(),
        },
        identity: None,
    }
}

async fn b1_join_matrix(governed_run: bool, governed_input: bool) {
    let session = SessionId::new();
    let mut driver =
        crate::driver::EphemeralRuntimeDriver::new(LogicalRuntimeId::for_session(&session));
    let first = Input::Prompt(crate::input::PromptInput::new("initial contributor", None));
    let first_id = first.id().clone();
    let late = b1_steer_prompt();
    let late_id = late.id().clone();
    assert!(
        driver
            .accept_input(first)
            .await
            .expect("first accepted")
            .is_accepted()
    );
    assert!(
        driver
            .accept_input(late)
            .await
            .expect("late accepted before run")
            .is_accepted()
    );
    let shared = driver.shared_dsl_authority();
    let first_claim = input("matrix-requester-a");
    let late_claim = input("matrix-requester-b");
    let first_association = first_claim
        .header()
        .authority_association
        .as_ref()
        .expect("first claim");
    let late_association = late_claim
        .header()
        .authority_association
        .as_ref()
        .expect("late claim");
    assert_ne!(
        first_association.candidate().requester,
        late_association.candidate().requester
    );
    assert_ne!(
        first_association.batch_identity_bytes().expect("first key"),
        late_association.batch_identity_bytes().expect("late key")
    );
    {
        let mut owner = shared.lock().expect("actual owner");
        // The canonical guard sees the same encoded, qualified association
        // data as the native projection. This matrix is not a grant proof.
        for (id, bind, association) in [
            (&first_id, governed_run, first_association),
            (&late_id, governed_input, late_association),
        ] {
            if bind {
                let (binding, batch) = crate::input_authority::association_binding(association)
                    .expect("actual native encoding");
                dsl::MeerkatMachineMutator::apply(
                    &mut *owner,
                    dsl::MeerkatMachineInput::BindInputAuthority {
                        input_id: id.to_string(),
                        authority_binding: binding.expect("binding"),
                        authority_batch_key: batch.expect("batch"),
                    },
                )
                .expect("actual canonical association");
            }
        }
    }
    let run = meerkat_core::RunId::new();
    driver
        .contract_begin_run_authority(run.clone())
        .expect("actual run");
    driver
        .machine_realize_authorized_stage_batch(
            crate::meerkat_machine::driver::test_authorized_stage_for_run(
                vec![first_id.clone()],
                run.clone(),
            ),
        )
        .expect("stage only initial contributor");
    let mut owner = shared.lock().expect("actual owner");
    b1_enter_model_boundary(&mut owner, &run);
    assert_eq!(owner.state().turn_phase, dsl::TurnPhase::CallingLlm);
    assert_eq!(
        owner.state().input_phases.get(&late_id.to_string()),
        Some(&dsl::InputPhase::Queued)
    );
    assert_eq!(
        owner
            .state()
            .input_live_boundary_delivery
            .get(&late_id.to_string()),
        Some(&dsl::LiveBoundaryDelivery::DurableAppend)
    );
    let before = format!("{:?}", owner.state());
    let result = dsl::MeerkatMachineMutator::apply(
        &mut *owner,
        dsl::MeerkatMachineInput::JoinLiveBoundaryDurableAppend {
            run_id: dsl::RunId::from_domain(&run),
            input_id: late_id.to_string(),
        },
    );
    if governed_run || governed_input {
        assert!(
            result.is_err(),
            "a governed input or target run cannot acquire an untracked late contributor"
        );
        assert_eq!(
            format!("{:?}", owner.state()),
            before,
            "rejected join has no owner mutation"
        );
    } else {
        let effects = result.expect("legacy ungoverned live join remains available");
        assert!(
            effects
                .effects()
                .iter()
                .any(|effect| matches!(effect, dsl::MeerkatMachineEffect::RecordRunAssociation))
        );
        assert_eq!(
            owner
                .state()
                .input_run_associations
                .get(&late_id.to_string()),
            Some(&dsl::RunId::from_domain(&run))
        );
        assert_eq!(
            owner
                .state()
                .input_live_boundary_join_phase
                .get(&late_id.to_string()),
            Some(&dsl::LiveBoundaryJoinPhase::Published)
        );
        dsl::MeerkatMachineMutator::apply(
            &mut *owner,
            dsl::MeerkatMachineInput::ResolveLiveBoundaryDurableAppendJoin {
                run_id: dsl::RunId::from_domain(&run),
                input_id: late_id.to_string(),
                lane: dsl::InputLane::Steer,
                observation: dsl::LiveBoundaryJoinObservation::NotApplied,
            },
        )
        .expect("legacy unapplied join still settles");
        assert_eq!(
            owner.state().input_phases.get(&late_id.to_string()),
            Some(&dsl::InputPhase::Queued)
        );
        assert!(
            !owner
                .state()
                .input_run_associations
                .contains_key(&late_id.to_string())
        );
    }
    drop(owner);
    assert_eq!(
        driver.input_phase(&first_id),
        Some(crate::input_state::InputLifecycleState::Staged)
    );
}

#[tokio::test]
async fn b1_live_join_rejects_governed_input_into_ungoverned_run() {
    b1_join_matrix(false, true).await;
}
#[tokio::test]
async fn b1_live_join_rejects_ungoverned_input_into_governed_run() {
    b1_join_matrix(true, false).await;
}
#[tokio::test]
async fn b1_live_join_rejects_different_requester_into_governed_run() {
    b1_join_matrix(true, true).await;
}
#[tokio::test]
async fn b1_live_join_preserves_ungoverned_join_and_settlement() {
    b1_join_matrix(false, false).await;
}

#[tokio::test]
async fn b1_live_join_cannot_change_a_warm_governed_batch() {
    let (configuration, prompt, domain) = configuration();
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let first = fresh_input(&prompt, "warm-first", false);
    let first_id = first.id().clone();
    let mut late = fresh_input(&prompt, "warm-late", false);
    let ingress = Arc::clone(
        late.header()
            .ingress_context
            .as_ref()
            .expect("prior observation"),
    );
    let controller = ingress
        .controller_client()
        .expect("actual selected client")
        .clone();
    let Input::Prompt(ref mut late_prompt) = late else {
        panic!("prompt")
    };
    late_prompt
        .turn_metadata
        .get_or_insert_with(Default::default)
        .handling_mode = Some(meerkat_core::types::HandlingMode::Steer);
    late_prompt.typed_turn_appends = vec![b1_live_append()];
    late.header_mut().ingress_context = None;
    let exact = NativeIngressContext::from_trusted_ingress(
        &late,
        ingress.requester().clone(),
        ingress.ingress_actor().clone(),
        ingress.realm().clone(),
        ingress.authentication().clone(),
    )
    .expect("rebind changed exact input")
    .with_controller_client(&late, controller)
    .expect("actual client");
    let late = late.with_ingress_context(exact).expect("exact input");
    let late_id = late.id().clone();
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session)
            .expect("entry")
            .driver,
    );
    let run = meerkat_core::RunId::new();
    let (context, shared) = {
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        driver
            .accept_input(first)
            .await
            .expect("actual initial admission");
        driver
            .accept_input(late)
            .await
            .expect("actual pre-run steer admission");
        let context = driver
            .batch_work_authorization(&run, std::slice::from_ref(&first_id))
            .expect("actual batch")
            .expect("governed");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    vec![first_id],
                    run.clone(),
                ),
            )
            .expect("stage initial contributor only");
        (context, driver.shared_dsl_authority())
    };
    b1_enter_model_boundary(&mut shared.lock().expect("actual owner"), &run);
    let bound = binding(&context, &run, &domain);
    let prepared = context
        .authorization()
        .prepare(&bound)
        .expect("real selected grant and native owner");
    prepared
        .check_current(&bound)
        .expect("warm current check before join");
    let (rejected, unchanged) = {
        let mut owner = shared.lock().expect("actual owner");
        let before = format!("{:?}", owner.state());
        let result = dsl::MeerkatMachineMutator::apply(
            &mut *owner,
            dsl::MeerkatMachineInput::JoinLiveBoundaryDurableAppend {
                run_id: dsl::RunId::from_domain(&run),
                input_id: late_id.to_string(),
            },
        );
        (result.is_err(), format!("{:?}", owner.state()) == before)
    };
    let still_current = prepared.check_current(&bound).is_ok();
    assert!(
        rejected,
        "the canonical join must reject before a new contributor can evade an already prepared check (warm check: {still_current})"
    );
    assert!(
        unchanged,
        "no contributor, queue, completion or join fact changed"
    );
    assert!(
        still_current,
        "a refused join does not revoke the unchanged original work"
    );
    context
        .authorization()
        .prepare(&bound)
        .expect("unchanged work also prepares normally");
}

#[tokio::test]
async fn review_context_wait_cannot_cross_native_membership_or_run_end() {
    for finish_run in [false, true] {
        let (machine, session, input_id, run, context, domain) = accepted().await;
        let bound = binding(&context, &run, &domain);
        context
            .authorization()
            .read_review_context(&bound, None)
            .await
            .expect("unchanged actual owner permits original source read");
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session)
                .expect("entry")
                .driver,
        );
        let locked = driver.lock().await;
        let read = context.authorization().read_review_context(&bound, None);
        tokio::pin!(read);
        assert!(
            futures::poll!(&mut read).is_pending(),
            "held actual driver is the read barrier"
        );
        {
            let authority = locked.shared_dsl_authority();
            let mut owner = authority.lock().expect("actual generated owner");
            let transition = if finish_run {
                dsl::MeerkatMachineInput::RunCompleted {
                    run_id: dsl::RunId::from_domain(&run),
                }
            } else {
                dsl::MeerkatMachineInput::RollbackStaged {
                    input_id: input_id.to_string(),
                    lane: dsl::InputLane::Queue,
                }
            };
            dsl::MeerkatMachineMutator::apply(&mut *owner, transition)
                .expect("actual native owner change");
        }
        drop(locked);
        assert!(
            read.await.is_err(),
            "no original material crosses the stale awaited read"
        );
    }
}
