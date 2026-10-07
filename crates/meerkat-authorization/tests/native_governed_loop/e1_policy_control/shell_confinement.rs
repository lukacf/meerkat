//! Real Required ShellTool in the existing authenticated native E1 loop.
//! The private HTTP receiver selects a fixed mixed shell batch; it is not a
//! live-model judgment oracle or a substitute for a native authority owner.

use super::*;
use meerkat_core::confinement::{
    ConfinementSpec, FilesystemAccess, IpNetworkAccess, PathAccess, PlatformBaseline,
};
use meerkat_tools::builtin::shell::ShellConfinement;

const DENIED_COMMAND: &str = "printf entered > denied-entered; printf changed > ../outside";
const PERMITTED_COMMAND: &str = "cat permitted-value; printf sibling > permitted-entered";

fn shell_response() -> String {
    let mut events = vec![start_message()];
    for (index, call_id, command) in [
        (0, DENIED_CALL, DENIED_COMMAND),
        (1, PERMITTED_CALL, PERMITTED_COMMAND),
    ] {
        let arguments = json!({"command": command}).to_string();
        events.extend([
            json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":call_id,"name":"shell","input":{}}}),
            json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":arguments}}),
            json!({"type":"content_block_stop","index":index}),
        ]);
    }
    events.extend([
        json!({"type":"message_delta","usage":{"output_tokens":2},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ]);
    sse(events)
}

fn shell_domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "shell-work".into(),
    }
}

fn shell_ceiling() -> ExecutionRestrictions {
    let mut restrictions = ceiling("execute");
    restrictions.resource_domains = ExactRestriction::exact([shell_domain()]);
    restrictions
}

/// Both exact shell commands are admitted by the generated operation grant.
/// The application's command mapping does not impersonate syscall enforcement.
struct ShellOwner(HttpRecordOwner);

impl OperationPolicyOwner for ShellOwner {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        self.0
            .authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let AuthorizationOperation::Tool(facts) = &binding.facts().operation else {
            return self
                .0
                .authorize_operation(association, binding, purpose, now_ms);
        };
        let expected_command = match facts.call_id.as_ref() {
            DENIED_CALL => DENIED_COMMAND,
            PERMITTED_CALL => PERMITTED_COMMAND,
            _ => return Err(denied().into()),
        };
        let arguments: Value = serde_json::from_str(facts.arguments.get()).map_err(|_| denied())?;
        if purpose != LocalPolicyPurpose::Operation
            || facts.name.as_str() != "shell"
            || !matches!(facts.target, ToolAuthorizationTarget::Dispatcher(_))
            || arguments != json!({"command": expected_command})
        {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: action("execute"),
                resource_domain: shell_domain(),
                processor: ProcessorRef::Principal {
                    principal: principal("executor"),
                },
                audience: AudienceRef::Principal {
                    principal: principal("requester"),
                },
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
}

fn assert_shell_feedback(denied_text: &str, permitted_text: &str) {
    let status = denied_text.lines().next().expect("returned shell status");
    let exit_code: i32 = status
        .strip_prefix("exit code ")
        .and_then(|rest| rest.split_whitespace().next())
        .expect("the real foreground shell returned an exit code")
        .parse()
        .unwrap();
    assert_ne!(exit_code, 0, "forbidden write unexpectedly succeeded");
    assert!(denied_text.contains("[stderr]"));
    assert!(denied_text.contains("../outside"));
    assert!(permitted_text.starts_with("exit code 0 ("));
    assert!(
        permitted_text.ends_with("\nrecord-7 value"),
        "unexpected permitted shell output: {permitted_text:?}",
    );
    assert!(!permitted_text.contains("[stderr]"));
}

fn assert_wire_shell_feedback(body: &Value) {
    let messages = body["messages"].as_array().unwrap();
    let assistant = messages
        .iter()
        .find(|message| {
            message["role"] == "assistant"
                && message["content"].as_array().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .filter(|block| block["type"] == "tool_use")
                        .count()
                        == 2
                })
        })
        .expect("one real assistant message owns both shell calls");
    let calls: Vec<_> = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "tool_use")
        .collect();
    for (call, (call_id, command)) in calls.iter().zip([
        (DENIED_CALL, DENIED_COMMAND),
        (PERMITTED_CALL, PERMITTED_COMMAND),
    ]) {
        assert_eq!(call["id"], call_id);
        assert_eq!(call["name"], "shell");
        assert_eq!(call["input"], json!({"command": command}));
    }
    let results: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flat_map(|blocks| blocks.iter())
        .filter(|block| block["type"] == "tool_result")
        .collect();
    assert_eq!(results.len(), 2, "exactly one feedback result per sibling");
    assert_eq!(results[0]["tool_use_id"], DENIED_CALL);
    assert_eq!(results[1]["tool_use_id"], PERMITTED_CALL);
    // Stock CompositeDispatcher records successful return of ShellOutput.
    // A nonzero command exit is carried in that output, not a fabricated
    // prelaunch ToolError or authorization Refused observation.
    assert_ne!(results[0]["is_error"], true);
    assert_ne!(results[1]["is_error"], true);
    assert_shell_feedback(
        &wire_text(&results[0]["content"]),
        &wire_text(&results[1]["content"]),
    );
}

async fn exercise_required_shell(server: &Server) {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let work = work.canonicalize().unwrap();
    let outside = root.path().join("outside");
    std::fs::write(&outside, b"unchanged").unwrap();
    // The parent can open the exact canary for writing. A denied child write
    // cannot pass because of an independently read-only fixture file.
    drop(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&outside)
            .unwrap(),
    );
    std::fs::write(work.join("permitted-value"), b"record-7 value").unwrap();
    let requirement = ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.clone())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.clone())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: false,
    }
    .try_into()
    .unwrap();
    let factory = AgentFactory::new(root.path().join("sessions"))
        .project_root(&work)
        .builtins(false)
        .shell(true)
        .with_shell_confinement(ShellConfinement::Required { requirement });
    let mut config = Config::default();
    // Select the executable directly instead of macOS /bin/sh's mutable
    // /private/var/select/sh selector, which is outside the read grant.
    config.shell.program = "/bin/bash".into();
    config.shell.timeout_secs = 5;
    config.shell.security_mode = meerkat_core::SecurityMode::Unrestricted;
    // This fixture grants shell operations, not provider-hosted search.
    config.provider_tools.anthropic.web_search = false;
    let client = http_client(server);
    let selected = client.controller_model_selection().unwrap();
    let endpoint = format!("{}/v1/messages", server.base_url);
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("e1-shell-native-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .unwrap();
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("confined-shell"),
            principal("executor"),
            None,
            shell_ceiling(),
        )
        .unwrap();
    let expected_controller = controller.clone();
    let expected_operation = operation.clone();
    let expected_selection = selected.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        let candidate = claimed.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").unwrap()
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
            || candidate.target.logical_runtime != id(&runtime.to_string())
            || candidate.controller_grant_lineage != [expected_controller.clone()]
            || candidate.authority_basis
                != (WorkAuthorityBasis::GrantLineage {
                    lineage: vec![expected_operation.clone()],
                })
            || candidate.controller_model.as_ref() != Some(&expected_selection)
            || candidate.controller_ceiling != ceiling("infer")
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(ShellOwner(HttpRecordOwner {
                    selection: selected.clone(),
                    endpoint: endpoint.clone(),
                })),
            })
            .unwrap(),
    );
    let sessions = Arc::new(RecordingStore::default());
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(client);
    builder.default_session_store = Some(sessions.clone());
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: E1_MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                auth_binding: Some(selected.auth_binding().unwrap().clone()),
                shell_env: Some(HashMap::from([("PATH".into(), "/usr/bin:/bin".into())])),
                ..Default::default()
            }),
            labels: None,
        },
        false,
    )
    .await
    .unwrap();
    let session_id = created.session_id;
    let visible = service.live_visible_tool_defs(&session_id).await.unwrap();
    let shell = visible
        .iter()
        .find(|tool| tool.name == "shell")
        .expect("automatic factory composition exposes the real shell");
    assert_eq!(
        shell.provenance.as_ref().unwrap().kind,
        meerkat_core::ToolSourceKind::Shell,
    );
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .unwrap();
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .unwrap();
    assert!(pin.selection() == &selected);
    meerkat_auth_core::save_tokens_and_publish_lifecycle(
        meerkat_core::auth::ProviderAuthPersistence::new(
            Arc::new(meerkat_auth_core::EphemeralTokenStore::new()),
            Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
        ),
        machine.generated_auth_lease_handle(),
        selected.credential().clone(),
        meerkat_core::auth::PersistedTokens::api_key("synthetic-e1-loopback-only"),
    )
    .await
    .unwrap();
    let claims = association(
        &LogicalRuntimeId::for_session(&session_id),
        controller,
        operation,
        selected.clone(),
    );
    let mut prompt = PromptInput::new("Attempt the two confined shell actions", None);
    prompt.header.authority_association = Some(claims.clone());
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let evidence = Evidence::start(&session_id, &input_id);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::evidence("e1-shell-current-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    let input = input.with_ingress_context(current).unwrap();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .unwrap();
    let completion = completion.expect("existing native completion receipt");
    tokio::time::timeout(
        Duration::from_secs(20),
        server.receiver.second_request.notified(),
    )
    .await
    .expect("same native run sends real shell feedback to the same model");
    let bodies = server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    for body in &bodies {
        assert_eq!(body["model"], E1_MODEL);
        assert_eq!(body["stream"], true);
    }
    assert_wire_shell_feedback(&bodies[1]);
    assert_eq!(
        std::fs::read(work.join("denied-entered")).unwrap(),
        b"entered"
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");
    assert_eq!(
        std::fs::read(work.join("permitted-entered")).unwrap(),
        b"sibling"
    );
    let stored = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
        serde_json::to_value(&stored).unwrap()["authorization_audit"].clone(),
    )
    .unwrap();
    let run_id = audit.first().unwrap().observation.run_id.clone().unwrap();
    for record in &audit {
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, input_id);
        assert!(record.contributors[0].requester == principal("requester"));
        assert!(record.contributors[0].logical_executor == principal("executor"));
        assert!(record.contributors[0].represented_subject.is_none());
        assert_eq!(record.observation.run_id.as_ref(), Some(&run_id));
        assert!(matches!(&record.observation.execution_scope,
            OperationExecutionScope::RuntimeInput {
                owner_session_id, submitted_input_id, canonical_input_id, ..
            } if owner_session_id == &session_id
                && submitted_input_id == &input_id
                && canonical_input_id == &input_id));
        assert!(
            !matches!(
                record.observation.observation,
                AuditObservation::Refused { .. }
                    | AuditObservation::AuthorizationUnavailable { .. }
            ),
            "unexpected audit record: {record:?}",
        );
    }
    let mut shell_ids = Vec::new();
    for (call_id, command) in [
        (DENIED_CALL, DENIED_COMMAND),
        (PERMITTED_CALL, PERMITTED_COMMAND),
    ] {
        let prepared: Vec<_> = audit.iter().filter(|record| matches!(
            &record.observation.observation,
            AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Tool {
                    call_id: actual_call, tool_name, arguments_digest, owners,
                } if actual_call == call_id
                    && tool_name == "shell"
                    && arguments_digest == &EvidenceDigest::of_bytes(json!({"command": command}).to_string().as_bytes())
                    && owners.iter().any(|owner|
                        owner.authority_key == "root-dispatcher" && owner.owner_key == "shell"))
        )).collect();
        assert_eq!(prepared.len(), 1, "one exact shell operation preparation");
        let operation_id = prepared[0].observation.operation_id.clone();
        let observations: Vec<_> = audit
            .iter()
            .filter(|record| record.observation.operation_id == operation_id)
            .map(|record| &record.observation.observation)
            .collect();
        assert_eq!(observations.len(), 3);
        assert!(matches!(observations[0], AuditObservation::Prepared { .. }));
        assert!(matches!(observations[1], AuditObservation::Entry));
        assert!(matches!(
            observations[2],
            AuditObservation::Outcome {
                outcome: OperationObservedOutcome::ToolDispatchReturned {
                    result_is_error: false,
                    terminal_error: None,
                    ..
                }
            }
        ));
        shell_ids.push(operation_id);
    }
    assert_ne!(shell_ids[0], shell_ids[1]);
    let model_ids: Vec<_> = audit.iter().filter_map(|record| match &record.observation.observation {
        AuditObservation::Prepared { target, .. }
            if matches!(target.as_ref(), AuditTarget::Model(model)
                if model.model == E1_MODEL && model.wire_model == E1_MODEL && model.endpoint == endpoint) =>
        {
            Some(record.observation.operation_id.clone())
        }
        _ => None,
    }).collect();
    assert_eq!(model_ids.len(), 2);
    for model_id in &model_ids {
        assert_eq!(
            audit
                .iter()
                .filter(|record| &record.observation.operation_id == model_id
                    && matches!(record.observation.observation, AuditObservation::Entry))
                .count(),
            1
        );
    }
    evidence.write("observed.json", &json!({
        "checkpoint":"E1", "scenario":"required_shell_syscall", "status":"observed_before_completion",
        "session_id":session_id, "input_id":input_id, "run_id":run_id,
        "association":claims, "shell_operation_ids":shell_ids, "model_operation_ids":model_ids,
        "outside_canary":"unchanged", "denied_body_marker":"entered", "permitted_body_marker":"sibling", "audit":audit,
    }));
    server.receiver.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("same native shell run must complete: {outcome:?}")
    };
    assert_eq!(result.text, FINISHED);
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    let saved = sessions
        .0
        .lock()
        .unwrap()
        .get(&session_id)
        .cloned()
        .unwrap();
    let batches: Vec<_> = saved
        .messages()
        .windows(2)
        .filter_map(|pair| match pair {
            [
                Message::BlockAssistant(message),
                Message::ToolResults { results, .. },
            ] if message
                .tool_calls()
                .map(|call| call.id.to_string())
                .collect::<Vec<_>>()
                == [DENIED_CALL, PERMITTED_CALL] =>
            {
                Some(results)
            }
            _ => None,
        })
        .collect();
    assert_eq!(batches.len(), 1);
    let results = batches[0];
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_use_id, DENIED_CALL);
    assert_eq!(results[1].tool_use_id, PERMITTED_CALL);
    assert!(!results[0].is_error && !results[1].is_error);
    assert_shell_feedback(&results[0].text_content(), &results[1].text_content());
    assert_eq!(server.receiver.bodies.lock().unwrap().len(), 2);
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        2
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"unchanged");
    let completed = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    let final_audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
        serde_json::to_value(&completed).unwrap()["authorization_audit"].clone(),
    )
    .unwrap();
    assert!(
        final_audit.starts_with(&audit),
        "the native row preserves earlier exact observations"
    );
    for record in &final_audit {
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, input_id);
        assert!(record.contributors[0].requester == principal("requester"));
        assert!(record.contributors[0].logical_executor == principal("executor"));
        assert!(record.contributors[0].represented_subject.is_none());
        assert_eq!(record.observation.run_id.as_ref(), Some(&run_id));
        assert!(matches!(&record.observation.execution_scope,
            OperationExecutionScope::RuntimeInput {
                owner_session_id, submitted_input_id, canonical_input_id, ..
            } if owner_session_id == &session_id
                && submitted_input_id == &input_id
                && canonical_input_id == &input_id));
        assert!(!matches!(
            record.observation.observation,
            AuditObservation::Refused { .. } | AuditObservation::AuthorizationUnavailable { .. }
        ));
    }
    for model_id in &model_ids {
        assert_eq!(
            final_audit
                .iter()
                .filter(|record| &record.observation.operation_id == model_id
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Outcome {
                            outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                        }
                    ))
                .count(),
            1
        );
    }
    evidence.write(
        "completed.json",
        &json!({
            "checkpoint":"E1", "scenario":"required_shell_syscall", "status":"completed",
            "session_id":session_id, "input_id":input_id, "run_id":run_id,
            "selected_controller":selected, "endpoint":endpoint, "association":claims,
            "shell_operation_ids":shell_ids, "model_operation_ids":model_ids,
            "model_http_count":2, "authorization_count":2, "outside_canary":"unchanged",
            "result":{"text":result.text, "terminal_cause_kind":result.terminal_cause_kind},
            "audit_capture_phase":"completed_native_input_row", "audit":final_audit,
        }),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real macOS confinement and existing native E1 evidence directory required"]
async fn adr_e1_required_shell_syscall_denial_preserves_native_sibling_and_model_turn() {
    let mut server = Server::start_with_tool_response(shell_response()).await;
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise_required_shell(&server),
    ))
    .catch_unwind()
    .await;
    server.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("native Required shell test timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
