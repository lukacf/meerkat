//! A storeless native loop through the real grant, admission and Agent owners.
//! Only the transport/model responses and application resource semantics are
//! fixtures. No test WorkAuthorization or substitute grant policy is installed.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use meerkat::{AgentFactory, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization::work::HostAuthorizationClock;
use meerkat_authorization_contracts::audit::{
    AuditObservation, AuditTarget, StoredAuthorizationAuditObservation,
};
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, ExactRestriction, ExecutionRestrictions, ProcessorRef, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::agent::ToolDispatchContext;
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationObservedOutcome, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    ToolAuthorizationTarget,
};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::service::{CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::{
    AgentError, AgentSessionStore, AgentToolDispatcher, Config, ControllerModelSelection, Message,
    PrincipalKind, PrincipalRef, Provider, Session, SessionId, SessionLlmIdentity, StopReason,
    ToolCallView, ToolDef, ToolDispatchOutcome, ToolError, ToolResult, TrustDomainId,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PreparedLlmRequest,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::input_authority::NativeIngressContext;
use meerkat_runtime::meerkat_machine::{
    MeerkatMachine, NativeGrantWorkConfiguration, NativeIngressCheck,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use serde::Deserialize;
use tokio::sync::Notify;

const MODEL: &str = "claude-sonnet-4-5";
const ENDPOINT: &str = "https://native-loop.invalid/messages";
const DENIED_CALL: &str = "attempt-delete";
const PERMITTED_CALL: &str = "attempt-read";

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("native-loop").expect("domain"),
    )
    .expect("qualified fixture principal")
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: principal("ingress"),
                namespace: "observations".into(),
            },
            resource_id: value.into(),
        },
        revision: id("observation-1"),
        digest: EvidenceDigest::from_array([1; 32]),
    }
}

fn identity() -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

fn credential() -> AuthCredentialIdentity {
    serde_json::from_value(
        serde_json::json!({"realm":"native-loop", "account":"scripted-controller"}),
    )
    .expect("nonsecret identity of the actual fixture transport")
}

fn selection() -> ControllerModelSelection {
    ControllerModelSelection::new(
        identity(),
        credential(),
        "scripted-profile".into(),
        "scripted-transport".into(),
    )
}

fn model_facts() -> ModelAuthorizationFacts {
    ModelAuthorizationFacts {
        identity: Arc::new(identity()),
        wire_model: MODEL.into(),
        hosted_capabilities: Arc::from([]),
        backend_profile_id: Some("scripted-profile".into()),
        backend_kind: "scripted-transport".into(),
        endpoint: ENDPOINT.into(),
        credential: Some(credential()),
        usage: ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

fn action(value: &str) -> ActionRef {
    ActionRef {
        feature: "native-loop".into(),
        action: value.into(),
    }
}

fn domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "records".into(),
    }
}

fn ceiling(action_name: &str) -> ExecutionRestrictions {
    let mut bounds = ExecutionRestrictions::unrestricted();
    bounds.actions = ExactRestriction::exact([action(action_name)]);
    bounds.resource_domains = ExactRestriction::exact([domain()]);
    bounds.processors = ExactRestriction::exact([ProcessorRef::Principal {
        principal: principal("executor"),
    }]);
    bounds.audiences = ExactRestriction::exact([AudienceRef::Principal {
        principal: principal("requester"),
    }]);
    bounds
}

/// Configured application entitlement only. NativeAcceptedWorkOwner still
/// proves exact retained rows/run custody before invoking this application hook,
/// and GrantBackedWorkPolicy resolves the actual generated grant separately.
struct InvocationOwner;
impl AdmittedWorkPolicyOwner for InvocationOwner {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        let candidate = association.candidate();
        if candidate.requester != principal("requester")
            || candidate.ingress_actor != principal("ingress")
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
        {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordArguments {
    record: String,
}

/// This owner maps BOTH read and delete to their actual correlated facts.
/// It deliberately does not deny delete: only the generated read-only grant
/// rejects the model's forbidden attempt.
struct RecordOwner;
impl OperationPolicyOwner for RecordOwner {
    fn authorize_controller_admission(
        &self,
        _: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        if facts.selection() != &selection()
            || facts.endpoint() != ENDPOINT
            || facts.wire_model() != MODEL
        {
            return Err(denied().into());
        }
        Ok(
            meerkat_authorization::grant_policy::ControllerAdmissionAllowance {
                operation_values: vec![LocalOperationValues {
                    action: action("infer"),
                    resource_domain: domain(),
                    processor: ProcessorRef::Principal {
                        principal: principal("executor"),
                    },
                    audience: AudienceRef::Principal {
                        principal: principal("requester"),
                    },
                }],
                restrictions: ExecutionRestrictions::unrestricted(),
            },
        )
    }

    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let verb = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if purpose == LocalPolicyPurpose::Controller
                    && selection().matches_model_facts(facts)
                    && facts.endpoint.as_ref() == ENDPOINT
                    && facts.wire_model.as_ref() == MODEL
                    && facts.hosted_capabilities.is_empty()
                    && facts.live_channel.is_none() =>
            {
                "infer"
            }
            AuthorizationOperation::Tool(facts) if purpose == LocalPolicyPurpose::Operation => {
                if !matches!(facts.target, ToolAuthorizationTarget::Dispatcher(_)) {
                    return Err(denied().into());
                }
                let arguments: RecordArguments =
                    serde_json::from_str(facts.arguments.get()).map_err(|_| denied())?;
                if arguments.record != "record-7" {
                    return Err(denied().into());
                }
                match facts.name.as_str() {
                    "read_record" => "read",
                    "delete_record" => "delete",
                    _ => return Err(denied().into()),
                }
            }
            _ => return Err(denied().into()),
        };
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: action(verb),
                resource_domain: domain(),
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

#[derive(Default)]
struct RecordingStore(Mutex<HashMap<SessionId, Session>>);
#[async_trait]
impl AgentSessionStore for RecordingStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.0
            .lock()
            .expect("fixture session store")
            .insert(session.id().clone(), session.clone());
        Ok(())
    }
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        Ok(SessionId::parse(id).ok().and_then(|id| {
            self.0
                .lock()
                .expect("fixture session store")
                .get(&id)
                .cloned()
        }))
    }
}

#[derive(Default)]
struct RecordingTools(Mutex<Vec<String>>);
#[async_trait]
impl AgentToolDispatcher for RecordingTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["delete_record", "read_record"]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "fixture record operation",
                    serde_json::json!({
                        "type":"object", "properties":{"record":{"type":"string"}},
                        "required":["record"], "additionalProperties":false,
                    }),
                ))
            })
            .collect()
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0
            .lock()
            .expect("entered bodies")
            .push(call.name.into());
        Ok(ToolResult::new(call.id.into(), "record-7 value".into(), false).into())
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        assert!(
            context.work_authorization().is_some(),
            "actual native context reaches tool entry"
        );
        self.dispatch(call).await
    }
}

#[derive(Default)]
struct ScriptedProvider {
    requests: Mutex<Vec<Vec<Message>>>,
    ready_to_finish: Notify,
    finish: Notify,
}

fn done(stop_reason: StopReason) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent::Done {
        outcome: LlmDoneOutcome::Success { stop_reason },
    })
}

fn tool_events(call: &str, name: &str) -> Vec<Result<LlmEvent, LlmError>> {
    vec![
        Ok(LlmEvent::ToolCallComplete {
            id: call.into(),
            name: name.into(),
            args: serde_json::json!({"record":"record-7"}),
            meta: None,
        }),
        done(StopReason::ToolUse),
    ]
}

fn tool_feedback(messages: &[Message], call_id: &str) -> Option<ToolResult> {
    messages.iter().find_map(|message| match message {
        Message::ToolResults { results, .. } => results
            .iter()
            .find(|result| result.tool_use_id == call_id)
            .cloned(),
        _ => None,
    })
}

#[async_trait]
impl LlmClient for ScriptedProvider {
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
        // The real LlmClientAdapter carries native request authorization. This
        // scripted physical transport supplies its own exact target, checks it
        // immediately before entry, and records its actual returned response.
        let check = request
            .authorization()
            .expect("native work reaches actual provider")
            .prepare(model_facts())
            .expect("actual native/grant controller authorization")
            .current()
            .expect("final controller entry check");
        check
            .observe_entry()
            .expect("native audit stages actual transport entry");
        check
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
            .expect("native audit stages fixture response headers");
        let mut requests = self.requests.lock().expect("request observations");
        let index = requests.len();
        requests.push(request.request().messages.clone());
        match index {
            0 => Box::pin(futures::stream::iter(tool_events(
                DENIED_CALL,
                "delete_record",
            ))),
            1 => {
                let feedback = tool_feedback(&requests[index], DENIED_CALL)
                    .expect("denial returned to same model loop");
                assert!(
                    feedback.is_error,
                    "forbidden body produced ordinary error feedback"
                );
                assert!(
                    feedback
                        .text_content()
                        .contains("operation unavailable under current authorization")
                );
                Box::pin(futures::stream::iter(tool_events(
                    PERMITTED_CALL,
                    "read_record",
                )))
            }
            2 => {
                let feedback = tool_feedback(&requests[index], PERMITTED_CALL)
                    .expect("permitted result returned to model");
                assert!(!feedback.is_error);
                assert_eq!(feedback.text_content(), "record-7 value");
                // Keep the actual run alive while the test reads its real row.
                // This avoids mistaking later archival/cleanup for audit loss.
                Box::pin(
                    futures::stream::once(async move {
                        self.ready_to_finish.notify_one();
                        self.finish.notified().await;
                        Ok(LlmEvent::TextDelta {
                            delta: "read completed after refusal".into(),
                            meta: None,
                        })
                    })
                    .chain(futures::stream::iter([done(StopReason::EndTurn)])),
                )
            }
            _ => panic!("unexpected extra model request"),
        }
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn association(
    runtime: &LogicalRuntimeId,
    controller: GrantLineageRef,
    operation: GrantLineageRef,
    selected: ControllerModelSelection,
) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: principal("requester"),
        ingress_actor: principal("ingress"),
        represented_subject: None,
        original_authentication: evidence("original-authentication"),
        logical_executor: principal("executor"),
        target: NativeWorkTarget {
            logical_owner: principal("native-owner"),
            logical_runtime: id(&runtime.to_string()),
            context: id("native-context"),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: principal("requester"),
            },
        },
        original_work: OriginalWorkRef {
            authority: principal("ingress"),
            work: id("work-7"),
        },
        root_event: evidence("original-event"),
        contributing_work: Vec::new(),
        authority_basis: WorkAuthorityBasis::GrantLineage {
            lineage: vec![operation],
        },
        controller_grant_lineage: vec![controller],
        controller_model: Some(selected),
        controller_ceiling: ceiling("infer"),
        // The operation's admitted ceiling is broad enough to map delete.
        // Denial must come from the separately resolved read-only grant.
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: RealmId::parse("native-loop").expect("realm"),
            ingress: ResourceDomain {
                authority: principal("ingress"),
                namespace: "prompts".into(),
            },
            occurrence_scope: id("occurrence-7"),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("immutable association claims")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_grant_denial_reaches_controller_and_permitted_tool_finishes_same_native_run() {
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
        .expect("actual controller lineage with unrestricted lifetime");
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("actual read-only operation grant");
    let expected_controller = controller.clone();
    let expected_operation = operation.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        let candidate = claimed.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").expect("realm")
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
            || candidate.target.logical_runtime != id(&runtime.to_string())
            || candidate.controller_grant_lineage != [expected_controller.clone()]
            || candidate.authority_basis
                != (WorkAuthorityBasis::GrantLineage {
                    lineage: vec![expected_operation.clone()],
                })
            || candidate.controller_model.as_ref() != Some(&selection())
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
                operation_owner: Arc::new(RecordOwner),
            })
            .expect("exclusive actual native host setup"),
    );
    let provider = Arc::new(ScriptedProvider::default());
    let tools = Arc::new(RecordingTools::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(provider.clone());
    builder.default_tool_dispatcher = Some(tools.clone());
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
    assert!(pin.selection() == &selection());
    // This scripted transport has no physical vault or external credential.
    // Native readiness still uses the actual machine's generated owner and the
    // exact credential identity pinned by the real adapter.
    let credential_owner = machine.generated_auth_lease_handle();
    meerkat_core::publish_token_lifecycle_acquired_for_identity(
        &credential_owner,
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-loop-credential"),
    )
    .expect("actual generated fixture credential before native admission");
    assert_eq!(
        credential_owner
            .resolve_credential_use_admission(
                &meerkat_core::handles::LeaseKey::from_credential_identity(
                    pin.selection().credential()
                ),
                meerkat_core::handles::CredentialUseIntent::HoldAuthority,
            )
            .expect("actual generated readiness"),
        meerkat_core::handles::CredentialUseDisposition::Authorized
    );
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let mut prompt = PromptInput::new("Try delete, then read if the operation is refused", None);
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
    .expect("separate current trusted ingress observation")
    .with_controller_client(&input, pin)
    .expect("bind real pin before native acceptance");
    let input = input
        .with_ingress_context(current)
        .expect("exact finalized input binding");
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("real native admission");
    let completion = completion.expect("actual native completion waiter");
    tokio::time::timeout(Duration::from_secs(20), provider.ready_to_finish.notified())
        .await
        .expect("same native run reached third controller request after denial and read");

    assert_eq!(*tools.0.lock().expect("body entries"), ["read_record"]);
    let stored = machine
        .input_state(&session_id, &input_id)
        .await
        .expect("actual native row query")
        .expect("current run retains contributor row");
    let wire = serde_json::to_value(&stored).expect("protected native row serialization");
    let audit: Vec<StoredAuthorizationAuditObservation> =
        serde_json::from_value(wire["authorization_audit"].clone())
            .expect("actual row-bound audit observations");
    assert!(!audit.is_empty());
    let run_id = audit[0]
        .observation
        .run_id
        .clone()
        .expect("actual admitted run");
    for record in &audit {
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, input_id);
        assert!(record.contributors[0].requester == principal("requester"));
        assert!(record.contributors[0].logical_executor == principal("executor"));
        assert!(record.contributors[0].represented_subject.is_none());
        assert_eq!(record.observation.run_id.as_ref(), Some(&run_id));
        assert!(
            matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput {
            owner_session_id, submitted_input_id, canonical_input_id, ..
        } if owner_session_id == &session_id && submitted_input_id == &input_id && canonical_input_id == &input_id)
        );
    }
    let denied_attempt = audit
        .iter()
        .find(|record| {
            matches!(&record.observation.observation,
        AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == DENIED_CALL && tool_name == "delete_record"))
        })
        .expect("native audit identifies denied delete attempt");
    assert!(
        !audit.iter().any(|record| record.observation.operation_id
            == denied_attempt.observation.operation_id
            && matches!(
                record.observation.observation,
                AuditObservation::Entry | AuditObservation::Outcome { .. }
            )),
        "denied attempt never became a physical entry or outcome"
    );
    let allowed_attempt = audit
        .iter()
        .find(|record| {
            matches!(&record.observation.observation,
        AuditObservation::Prepared { target, .. }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == PERMITTED_CALL && tool_name == "read_record"))
        })
        .expect("native audit identifies permitted read preparation");
    assert!(audit.iter().any(|record| record.observation.operation_id
        == allowed_attempt.observation.operation_id
        && matches!(record.observation.observation, AuditObservation::Entry)));
    assert!(audit.iter().any(|record| record.observation.operation_id
        == allowed_attempt.observation.operation_id
        && matches!(
            &record.observation.observation,
            AuditObservation::Outcome {
                outcome: OperationObservedOutcome::ToolDispatchReturned {
                    result_is_error: false,
                    terminal_error: None,
                    ..
                }
            }
        )));

    provider.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .expect("native run completes")
        .expect("native completion observation");
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("ordinary refusal must not terminate admitted work: {outcome:?}")
    };
    assert_eq!(result.text, "read completed after refusal");
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    assert_eq!(provider.requests.lock().expect("requests").len(), 3);
    assert_eq!(*tools.0.lock().expect("body entries"), ["read_record"]);
}

#[path = "native_governed_loop/observation_deferred_siblings.rs"]
mod observation_deferred_siblings;

#[path = "native_governed_loop/e1_policy_control.rs"]
mod e1_policy_control;

#[path = "native_governed_loop/e3_tool_settlement.rs"]
mod e3_tool_settlement;
