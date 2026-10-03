//! Storeless native cost fixtures with ordinary correctness tests and opt-in timing.
//! Transport and application resource semantics are fixtures; authorization,
//! grants, currentness, native rows, execution and audit use actual owners.
//!
//! Build one optimized test binary and use Cargo's emitted executable path:
//! ```text
//! ./scripts/repo-cargo test --locked --release -p meerkat-authorization \
//!   --test native_cost --no-run --message-format=json
//! "$BINARY" --exact native_cost_correctness --nocapture --test-threads=1
//! "$BINARY" --exact representative::native_representative_correctness \
//!   --nocapture --test-threads=1
//! ```
//! Measure only in an explicitly allocated, externally monitored quiet window.
//! Set NATIVE_COST_RUN=approved-quiet-window, NATIVE_COST_WARMUP_PAIRS=100,
//! NATIVE_COST_PAIRS=2000 and a fresh NATIVE_COST_OUTPUT path for each matrix.
//! Run that same binary with --exact native_cost_matrix, then with
//! --exact representative::native_representative_matrix. Both matrices require
//! --ignored --nocapture --test-threads=1 and separate raw outputs. The
//! representative matrix is expensive; allocate its wall budget explicitly
//! rather than borrowing a smoke timeout.
//! Preserve the raw sample JSON beside the existing command/stdout/stderr logs.
//! These diagnostics do not establish full/default overhead acceptance.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    OperationAuthorizationError, OperationObservedOutcome, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, ToolAuthorizationTarget,
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
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

const MODEL: &str = "claude-sonnet-4-5";
const ENDPOINT: &str = "https://native-loop.invalid/messages";
const DENIED_CALL: &str = "attempt-delete";
const READ_CALLS: [&str; 4] = ["read-0", "read-1", "read-2", "read-3"];
const FILE_BYTES: &[u8] = b"native-cost-record-7\n";

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
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
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
        OperationAuthorizationError,
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
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
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

fn association(
    runtime: &LogicalRuntimeId,
    controller: Vec<GrantLineageRef>,
    operation: Vec<GrantLineageRef>,
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
        authority_basis: WorkAuthorityBasis::GrantLineage { lineage: operation },
        controller_grant_lineage: controller,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum HostMode {
    TrustedHost,
    LocalGoverned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Instrumentation {
    TurnOnly,
    Boundaries,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Script {
    Allowed,
    DeniedDelete,
    RevokeBeforeRead,
}

#[derive(Default)]
struct SessionStore(Mutex<HashMap<SessionId, Session>>);
#[async_trait]
impl AgentSessionStore for SessionStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.0
            .lock()
            .expect("session store")
            .insert(session.id().clone(), session.clone());
        Ok(())
    }
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        Ok(SessionId::parse(id)
            .ok()
            .and_then(|id| self.0.lock().expect("session store").get(&id).cloned()))
    }
}

struct FileTools {
    path: PathBuf,
    mode: HostMode,
    script: Script,
    reads: AtomicUsize,
    deletes: AtomicUsize,
    before_read: Notify,
    release_read: Notify,
}
#[async_trait]
impl AgentToolDispatcher for FileTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["read_record", "delete_record"]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "fixed local fixture record operation",
                    serde_json::json!({
                        "type":"object", "properties":{"record":{"type":"string"}},
                        "required":["record"], "additionalProperties":false,
                    }),
                ))
            })
            .collect()
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("fixture requires actual resolved dispatch context")
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        assert_eq!(
            context.work_authorization().is_some(),
            self.mode == HostMode::LocalGoverned
        );
        let args: RecordArguments =
            serde_json::from_str(call.args.get()).expect("exact tool arguments");
        assert_eq!(args.record, "record-7");
        if self.script == Script::RevokeBeforeRead && call.name == "read_record" {
            // Functional control only, never a timing cell. The actual adapter
            // rechecks the same borrowed call after its awaited preparation.
            self.before_read.notify_one();
            self.release_read.notified().await;
            let refreshed = context.check_tool_authorization(call, Some(plan))?;
            let _retained = refreshed.as_ref().unwrap_or(context);
        }
        match call.name {
            "read_record" => {
                let bytes = std::fs::read(&self.path).expect("read actual fixture file");
                assert_eq!(bytes, FILE_BYTES);
                self.reads.fetch_add(1, Ordering::Relaxed);
                Ok(ToolResult::new(call.id.into(), "record-7 value".into(), false).into())
            }
            "delete_record" => {
                self.deletes.fetch_add(1, Ordering::Relaxed);
                std::fs::remove_file(&self.path).expect("delete would be an actual effect");
                Ok(ToolResult::new(call.id.into(), "deleted".into(), false).into())
            }
            _ => panic!("unexpected fixture tool"),
        }
    }
}

#[derive(Default)]
struct BoundarySamples {
    model_authorization_ns: Vec<u64>,
    tool_batch_start: Option<Instant>,
    tool_batch_ns: Option<u64>,
}
struct ProviderFixture {
    mode: HostMode,
    script: Script,
    instrumentation: Instrumentation,
    requests: AtomicUsize,
    boundaries: Mutex<BoundarySamples>,
    // Correctness inspection is done only in untimed smoke. Performance
    // iterations have no inspection pause and no concurrent row reader.
    pause_for_inspection: bool,
    final_ready: Notify,
    finish: Notify,
}

fn ns(duration: Duration) -> u64 {
    duration.as_nanos().try_into().expect("finite nanoseconds")
}
fn done(reason: StopReason) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent::Done {
        outcome: LlmDoneOutcome::Success {
            stop_reason: reason,
        },
    })
}
fn tool(call: &str, name: &str) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent::ToolCallComplete {
        id: call.into(),
        name: name.into(),
        args: serde_json::json!({"record":"record-7"}),
        meta: None,
    })
}
fn feedback<'a>(messages: &'a [Message], call: &str) -> &'a ToolResult {
    messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResults { results, .. } => results.iter().find(|r| r.tool_use_id == call),
            _ => None,
        })
        .expect("every emitted tool call has a matching actual transcript result")
}

#[async_trait]
impl LlmClient for ProviderFixture {
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
        panic!("both modes must use the same actual prepared adapter route")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let index = self.requests.fetch_add(1, Ordering::Relaxed);
        assert!(
            index < 2,
            "extra model requests cannot masquerade as a fast turn"
        );
        if index == 1 && self.instrumentation == Instrumentation::Boundaries {
            let end = Instant::now();
            let mut boundaries = self.boundaries.lock().expect("boundary recorder");
            let start = boundaries
                .tool_batch_start
                .take()
                .expect("actual ToolUse boundary polled");
            boundaries.tool_batch_ns = Some(ns(end.duration_since(start)));
        }
        let start = (self.instrumentation == Instrumentation::Boundaries).then(Instant::now);
        match self.mode {
            HostMode::TrustedHost => assert!(
                request.authorization().is_none(),
                "baseline has no substitute authorization policy"
            ),
            HostMode::LocalGoverned => {
                let prepared = request
                    .authorization()
                    .expect("native per-work companion")
                    .prepare(model_facts())
                    .expect("actual generated-grant preparation");
                let current = prepared.current().expect("current before Entry");
                current.observe_entry().expect("actual native Entry append");
                let current = current
                    .current()
                    .expect("current after Entry before scripted transport");
                // A single scripted response replaces only external transport.
                // It retains the actual Prepared/Entry/Outcome and both checks.
                current
                    .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
                    .expect("actual native Outcome append");
            }
        }
        if let Some(start) = start {
            let elapsed = ns(start.elapsed());
            self.boundaries
                .lock()
                .expect("boundary recorder")
                .model_authorization_ns
                .push(elapsed);
        }
        if index == 0 {
            let mut events = Vec::with_capacity(6);
            match self.script {
                Script::Allowed => events.extend(READ_CALLS.map(|id| tool(id, "read_record"))),
                Script::DeniedDelete => {
                    events.push(tool(DENIED_CALL, "delete_record"));
                    events.extend(READ_CALLS.map(|id| tool(id, "read_record")));
                }
                Script::RevokeBeforeRead => events.push(tool(READ_CALLS[0], "read_record")),
            }
            events.push(done(StopReason::ToolUse));
            Box::pin(futures::stream::iter(events).inspect(move |event| {
                if self.instrumentation == Instrumentation::Boundaries
                    && matches!(
                        event,
                        Ok(LlmEvent::Done {
                            outcome: LlmDoneOutcome::Success {
                                stop_reason: StopReason::ToolUse
                            }
                        })
                    )
                {
                    // The recorder lock is acquired before the start timestamp.
                    // A fixed Option assignment/drop follows it. This observer
                    // cost is matched in both modes, never subtracted as zero.
                    let mut recorder = self.boundaries.lock().expect("boundary recorder");
                    recorder.tool_batch_start = Some(Instant::now());
                }
            }))
        } else {
            match self.script {
                Script::Allowed | Script::DeniedDelete => {
                    for id in READ_CALLS {
                        let result = feedback(&request.request().messages, id);
                        assert!(!result.is_error);
                        assert_eq!(result.text_content(), "record-7 value");
                        assert!(result.settlement_failures.is_empty());
                    }
                    if self.script == Script::DeniedDelete {
                        let result = feedback(&request.request().messages, DENIED_CALL);
                        assert!(result.is_error);
                        assert!(
                            result
                                .text_content()
                                .contains("operation unavailable under current authorization")
                        );
                    }
                }
                Script::RevokeBeforeRead => {
                    assert!(feedback(&request.request().messages, READ_CALLS[0]).is_error);
                }
            }
            Box::pin(
                futures::stream::once(async move {
                    if self.pause_for_inspection {
                        self.final_ready.notify_one();
                        self.finish.notified().await;
                    }
                    Ok(LlmEvent::TextDelta {
                        delta: if self.script == Script::Allowed {
                            "four reads completed"
                        } else {
                            "control completed"
                        }
                        .into(),
                        meta: None,
                    })
                })
                .chain(futures::stream::iter([done(StopReason::EndTurn)])),
            )
        }
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn issue_lineage(
    grants: &LocalGrantAuthority,
    label: &str,
    verb: &str,
    depth: usize,
) -> Vec<GrantLineageRef> {
    assert!((1..=3).contains(&depth));
    let root = grants
        .issue_root(
            &principal("grant-owner"),
            id(&format!("{label}-0")),
            principal("executor"),
            None,
            ceiling(verb),
        )
        .expect("real root issuance");
    let mut chain = vec![root];
    for index in 1..depth {
        let next = grants
            .issue_child(
                &principal("executor"),
                chain.last().unwrap(),
                id(&format!("{label}-{index}")),
                principal("executor"),
                ceiling(verb),
            )
            .expect("real child issuance through generated owner");
        chain.push(next);
    }
    chain
}

struct Fixture {
    mode: HostMode,
    machine: Arc<MeerkatMachine>,
    service: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
    provider: Arc<ProviderFixture>,
    tools: Arc<FileTools>,
    session_id: SessionId,
    input: Input,
    grants: Option<Arc<LocalGrantAuthority>>,
    operation: Vec<GrantLineageRef>,
}

impl Fixture {
    async fn new(
        mode: HostMode,
        depth: usize,
        instrumentation: Instrumentation,
        script: Script,
        inspect: bool,
        path: PathBuf,
    ) -> Self {
        let (machine, grants, controller, operation) = if mode == HostMode::LocalGoverned {
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
            let controller = issue_lineage(&grants, "controller", "infer", depth);
            let operation = issue_lineage(&grants, "read", "read", depth);
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
                    || candidate.controller_grant_lineage != expected_controller
                    || candidate.authority_basis
                        != (WorkAuthorityBasis::GrantLineage {
                            lineage: expected_operation.clone(),
                        })
                    || candidate.controller_model.as_ref() != Some(&selection())
                    || candidate.controller_ceiling != ceiling("infer")
                {
                    return Err(denied().into());
                }
                Ok(())
            });
            let machine = MeerkatMachine::ephemeral()
                .with_local_grant_authorization(NativeGrantWorkConfiguration {
                    grants: Arc::clone(&grants),
                    ingress,
                    invocation_owner: Arc::new(InvocationOwner),
                    operation_owner: Arc::new(RecordOwner),
                })
                .expect("real local-governed host");
            (machine, Some(grants), controller, operation)
        } else {
            // The actual no-installed-host/None-input route is the trusted host
            // baseline. No always-allow WorkAuthorization exists in this fixture.
            (MeerkatMachine::ephemeral(), None, Vec::new(), Vec::new())
        };
        let machine = Arc::new(machine);
        let provider = Arc::new(ProviderFixture {
            mode,
            script,
            instrumentation,
            requests: AtomicUsize::new(0),
            boundaries: Mutex::new(BoundarySamples {
                model_authorization_ns: Vec::with_capacity(2),
                ..Default::default()
            }),
            pause_for_inspection: inspect,
            final_ready: Notify::new(),
            finish: Notify::new(),
        });
        let tools = Arc::new(FileTools {
            path,
            mode,
            script,
            reads: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            before_read: Notify::new(),
            release_read: Notify::new(),
        });
        let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
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
        .expect("same native actor setup in both modes");
        let session_id = created.session_id;
        // Both modes acquire the same actual immutable adapter child. Only the
        // governed input installs it as a controller with real retained claims.
        let actor = service
            .live_session_actor_witness(&session_id)
            .await
            .expect("actual actor");
        let pin = service
            .pin_controller_client_for_actor(&actor)
            .await
            .expect("actual runnable selected child");
        assert_eq!(pin.selection(), &selection());
        // Both modes have the same actual generated credential setup outside
        // timing. This scripted transport has no vault or external account.
        // Use the exact actor-pinned identity, never a caller-selected allow flag.
        let credential_owner = machine.generated_auth_lease_handle();
        meerkat_core::publish_token_lifecycle_acquired_for_identity(
            &credential_owner,
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-cost-credential"),
        )
        .expect("actual fixture credential before native admission");
        assert_eq!(
            credential_owner
                .resolve_credential_use_admission(
                    &meerkat_core::handles::LeaseKey::from_credential_identity(
                        pin.selection().credential()
                    ),
                    meerkat_core::handles::CredentialUseIntent::HoldAuthority,
                )
                .expect("actual generated readiness"),
            meerkat_core::handles::CredentialUseDisposition::Authorized,
        );
        let mut prompt = PromptInput::new("Read record-7 four times and finish", None);
        if mode == HostMode::LocalGoverned {
            prompt.header.authority_association = Some(association(
                &LogicalRuntimeId::for_session(&session_id),
                controller,
                operation.clone(),
                pin.selection().clone(),
            ));
        }
        let mut input = Input::Prompt(prompt);
        if mode == HostMode::LocalGoverned {
            let current = NativeIngressContext::from_trusted_ingress(
                &input,
                principal("requester"),
                principal("ingress"),
                RealmId::parse("native-loop").expect("realm"),
                evidence("fresh-transport-authentication"),
            )
            .expect("actual exact-input ingress")
            .with_controller_client(&input, pin)
            .expect("bind actual pin");
            input = input
                .with_ingress_context(current)
                .expect("exact final input");
        }
        Self {
            mode,
            machine,
            service,
            provider,
            tools,
            session_id,
            input,
            grants,
            operation,
        }
    }
    async fn audit(&self) -> Vec<StoredAuthorizationAuditObservation> {
        let stored = self
            .machine
            .input_state(&self.session_id, self.input.id())
            .await
            .expect("actual native row query")
            .expect("accepted row must remain observable");
        let wire = serde_json::to_value(stored).expect("native owner snapshot");
        match wire.get("authorization_audit") {
            Some(records) => serde_json::from_value(records.clone()).expect("real row audit"),
            None if self.mode == HostMode::TrustedHost => Vec::new(),
            None => panic!("governed row omitted its actual audit"),
        }
    }
    async fn close(&self) {
        self.machine
            .retire_runtime(&self.session_id)
            .await
            .expect("real native retirement");
        self.service
            .try_shutdown()
            .await
            .expect("session shutdown request succeeds");
    }
}

fn assert_audit(fixture: &Fixture, audit: &[StoredAuthorizationAuditObservation], script: Script) {
    if fixture.mode == HostMode::TrustedHost {
        assert!(
            audit.is_empty(),
            "actual TrustedHost route has no invented local audit"
        );
        return;
    }
    assert!(
        !audit.is_empty(),
        "governed operations must execute their real audit path"
    );
    let run = audit[0]
        .observation
        .run_id
        .as_ref()
        .expect("actual native run");
    for record in audit {
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(&record.contributors[0].input_id, fixture.input.id());
        assert_eq!(record.contributors[0].requester, principal("requester"));
        assert_eq!(
            record.contributors[0].logical_executor,
            principal("executor")
        );
        assert_eq!(record.observation.run_id.as_ref(), Some(run));
        assert!(
            matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput {
            owner_session_id, submitted_input_id, canonical_input_id, ..
        } if owner_session_id == &fixture.session_id && submitted_input_id == fixture.input.id()
            && canonical_input_id == fixture.input.id())
        );
    }
    let prepared: Vec<_> = audit
        .iter()
        .filter(|r| matches!(r.observation.observation, AuditObservation::Prepared { .. }))
        .collect();
    let expected = if script == Script::RevokeBeforeRead {
        3
    } else {
        6
    };
    assert_eq!(
        prepared.len(),
        expected,
        "two model operations plus actual allowed tool preparations"
    );
    assert_eq!(prepared.iter().filter(|r| matches!(&r.observation.observation,
        AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Model(_)))).count(), 2);
    for record in &prepared {
        let operation = &record.observation.operation_id;
        assert_eq!(
            audit
                .iter()
                .filter(|r| &r.observation.operation_id == operation
                    && matches!(r.observation.observation, AuditObservation::Entry))
                .count(),
            1
        );
        assert_eq!(
            audit
                .iter()
                .filter(|r| &r.observation.operation_id == operation
                    && matches!(r.observation.observation, AuditObservation::Outcome { .. }))
                .count(),
            1
        );
    }
    if script != Script::RevokeBeforeRead {
        for call in READ_CALLS {
            let record = prepared.iter().find(|r| matches!(&r.observation.observation,
                AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
                    if call_id == call && tool_name == "read_record"))).expect("every successful read prepared");
            assert!(audit.iter().any(|r| r.observation.operation_id
                == record.observation.operation_id
                && matches!(
                    &r.observation.observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchReturned {
                            result_is_error: false,
                            terminal_error: None,
                            ..
                        }
                    }
                )));
        }
    }
    if script == Script::Allowed {
        assert_eq!(
            audit.len(),
            18,
            "no missing, extra or refused operations in timing cells"
        );
    } else {
        let call = if script == Script::DeniedDelete {
            DENIED_CALL
        } else {
            READ_CALLS[0]
        };
        let refusal = audit.iter().find(|r| matches!(&r.observation.observation,
            AuditObservation::Refused { target, .. } if matches!(target.as_ref(), AuditTarget::Tool { call_id, .. }
                if call_id == call))).expect("actual typed refusal recorded");
        if script == Script::DeniedDelete {
            assert!(!audit.iter().any(|r| r.observation.operation_id
                == refusal.observation.operation_id
                && matches!(
                    r.observation.observation,
                    AuditObservation::Entry | AuditObservation::Outcome { .. }
                )));
        } else {
            // Entry precedes the awaited recheck. Its refusal returns a typed
            // dispatch error before the read, then becomes model feedback.
            assert!(audit.iter().any(|r| r.observation.operation_id
                == refusal.observation.operation_id
                && matches!(
                    &r.observation.observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchError {
                            error: meerkat_core::ops::ToolDispatchTerminalErrorKind::AuthorizationRefused,
                        }
                    }
                )));
        }
    }
}

#[derive(Serialize)]
struct Sample {
    pair: usize,
    first_in_pair: bool,
    depth: usize,
    mode: HostMode,
    instrumentation: Instrumentation,
    turn_ns: u64,
    model_authorization_ns: Vec<u64>,
    tool_batch_ns: Option<u64>,
    model_requests: usize,
    read_effects: usize,
    audit_records: usize,
}

async fn sample(
    mode: HostMode,
    depth: usize,
    instrumentation: Instrumentation,
    path: PathBuf,
    pair: usize,
    first_in_pair: bool,
) -> Sample {
    let fixture = Fixture::new(mode, depth, instrumentation, Script::Allowed, false, path).await;
    let input = fixture.input.clone();
    // Fixture setup/inspection I/O, formatting, quantiles and statistical
    // recording are outside timing. The four intended file reads stay inside.
    // TurnOnly additionally disables all per-boundary clocks/recorders.
    let start = Instant::now();
    let (accepted, completion) = fixture
        .machine
        .accept_input_with_completion(&fixture.session_id, input)
        .await
        .expect("actual native admission");
    let outcome = completion
        .expect("same actual completion path")
        .wait()
        .await
        .expect("native completion");
    let elapsed = ns(start.elapsed());
    assert!(accepted.is_accepted());
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("timing requires successful same-run completion")
    };
    assert_eq!(result.text, "four reads completed");
    assert_eq!(result.session_id, fixture.session_id);
    assert!(result.terminal_cause_kind.is_none());
    assert_eq!(result.tool_calls, 4);
    assert_eq!(result.turns, 2);
    assert_eq!(fixture.provider.requests.load(Ordering::Relaxed), 2);
    assert_eq!(fixture.tools.reads.load(Ordering::Relaxed), 4);
    assert_eq!(fixture.tools.deletes.load(Ordering::Relaxed), 0);
    assert_eq!(
        std::fs::read(&fixture.tools.path).expect("file persists"),
        FILE_BYTES
    );
    let audit = fixture.audit().await;
    assert_audit(&fixture, &audit, Script::Allowed);
    let (models, batch) = {
        let record = fixture
            .provider
            .boundaries
            .lock()
            .expect("boundary recorder");
        if instrumentation == Instrumentation::TurnOnly {
            assert!(record.model_authorization_ns.is_empty());
            assert!(record.tool_batch_ns.is_none());
        } else {
            assert_eq!(record.model_authorization_ns.len(), 2);
            assert!(record.tool_batch_ns.is_some());
        }
        (record.model_authorization_ns.clone(), record.tool_batch_ns)
    };
    fixture.close().await;
    Sample {
        pair,
        first_in_pair,
        depth,
        mode,
        instrumentation,
        turn_ns: elapsed,
        model_authorization_ns: models,
        tool_batch_ns: batch,
        model_requests: 2,
        read_effects: 4,
        audit_records: audit.len(),
    }
}

fn fixture_file() -> PathBuf {
    // This creates only a new local temp file. No endpoint, credentials or
    // caller-selected production file is read, modified or deleted.
    let path = std::env::temp_dir().join(format!("meerkat-native-cost-{}.txt", SessionId::new()));
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("new fixture file");
    file.write_all(FILE_BYTES).expect("fixture bytes");
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_cost_correctness() {
    let path = fixture_file();
    for depth in [1, 3] {
        for mode in [HostMode::TrustedHost, HostMode::LocalGoverned] {
            // Exercise both timing configurations for correctness without
            // interpreting these smoke durations as performance evidence.
            for instrumentation in [Instrumentation::TurnOnly, Instrumentation::Boundaries] {
                tokio::time::timeout(
                    Duration::from_secs(30),
                    sample(mode, depth, instrumentation, path.clone(), 0, true),
                )
                .await
                .expect("bounded functional control");
            }
        }
    }
    for script in [Script::DeniedDelete, Script::RevokeBeforeRead] {
        let fixture = Fixture::new(
            HostMode::LocalGoverned,
            1,
            Instrumentation::TurnOnly,
            script,
            true,
            path.clone(),
        )
        .await;
        let (_, completion) = fixture
            .machine
            .accept_input_with_completion(&fixture.session_id, fixture.input.clone())
            .await
            .expect("actual governed control admission");
        if script == Script::RevokeBeforeRead {
            tokio::time::timeout(
                Duration::from_secs(20),
                fixture.tools.before_read.notified(),
            )
            .await
            .expect("actual adapter has prepared but not read");
            let mut custody = fixture
                .machine
                .try_controller_grant_mutation()
                .expect("real native mutation custody");
            fixture
                .grants
                .as_ref()
                .expect("real grants")
                .revoke(
                    &principal("grant-owner"),
                    fixture.operation.last().unwrap(),
                    &mut custody,
                )
                .expect("revoke actual operation grant");
            drop(custody);
            fixture.tools.release_read.notify_one();
        }
        tokio::time::timeout(
            Duration::from_secs(20),
            fixture.provider.final_ready.notified(),
        )
        .await
        .expect("retained controller receives actual tool feedback");
        assert_eq!(
            fixture.tools.reads.load(Ordering::Relaxed),
            if script == Script::DeniedDelete { 4 } else { 0 }
        );
        assert_eq!(fixture.tools.deletes.load(Ordering::Relaxed), 0);
        assert_eq!(
            std::fs::read(&path).expect("forbidden delete did not execute"),
            FILE_BYTES
        );
        assert_audit(&fixture, &fixture.audit().await, script);
        fixture.provider.finish.notify_one();
        let outcome = tokio::time::timeout(Duration::from_secs(20), completion.unwrap().wait())
            .await
            .expect("control completes")
            .expect("completion");
        let CompletionOutcome::Completed(result) = outcome else {
            panic!("refusal must preserve same run")
        };
        assert!(result.terminal_cause_kind.is_none());
        assert_eq!(fixture.provider.requests.load(Ordering::Relaxed), 2);
        fixture.close().await;
    }
    std::fs::remove_file(path).expect("remove fixture only");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit quiet-host performance lease required; never run during normal tests"]
async fn native_cost_matrix() {
    assert_eq!(
        std::env::var("NATIVE_COST_RUN").as_deref(),
        Ok("approved-quiet-window"),
        "explicit run mode required"
    );
    assert!(
        !std::hint::black_box(cfg!(debug_assertions)),
        "optimized binary required: generated debug invariants have different costs"
    );
    let count = |name: &str, default: usize| {
        std::env::var(name)
            .ok()
            .map(|v| v.parse::<usize>().expect("integer sample count"))
            .unwrap_or(default)
    };
    let warmup = count("NATIVE_COST_WARMUP_PAIRS", 100);
    let pairs = count("NATIVE_COST_PAIRS", 2000);
    assert!((20..=500).contains(&warmup));
    assert!(
        (2000..=10000).contains(&pairs),
        "do not report a p99 tail from a smoke sample"
    );
    let path = fixture_file();
    let mut samples = Vec::with_capacity(2 * 2 * 2 * pairs);
    for depth in [1, 3] {
        for instrumentation in [Instrumentation::TurnOnly, Instrumentation::Boundaries] {
            for iteration in 0..warmup + pairs {
                let order = if iteration % 2 == 0 {
                    [HostMode::TrustedHost, HostMode::LocalGoverned]
                } else {
                    [HostMode::LocalGoverned, HostMode::TrustedHost]
                };
                for (position, mode) in order.into_iter().enumerate() {
                    let value = tokio::time::timeout(
                        Duration::from_secs(30),
                        sample(
                            mode,
                            depth,
                            instrumentation,
                            path.clone(),
                            iteration.saturating_sub(warmup),
                            position == 0,
                        ),
                    )
                    .await
                    .expect("timed-out cells fail; never discard them as outliers");
                    if iteration >= warmup {
                        samples.push(value);
                    }
                }
            }
        }
    }
    std::fs::remove_file(path).expect("remove fixture only");
    let output = std::env::var_os("NATIVE_COST_OUTPUT").expect("raw measurement output path");
    let payload = serde_json::json!({
        "schema": 1, "samples": samples, "warmup_pairs": warmup, "pairs_per_cell": pairs,
        "status": "measured_subset_only", "failures": 0, "timeouts": 0,
        "unsupported": [
            {"cell":"representative", "contributors":4, "old_rows":252, "prefix_reads":333, "prefix_audit_records":1002,
             "reason":"measured separately by representative::native_representative_matrix"},
            {"boundary":"individual_tool_prepare_checks_audit", "reason":"no nanosecond full-dispatch observation seam; batch is not four individual samples"}
        ],
        "acceptance": "not_evaluated: full matrix and individual-operation measurement remain required"
    });
    use std::io::Write;
    let mut raw_output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .expect("create a new raw sample artifact without overwriting prior evidence");
    raw_output
        .write_all(&serde_json::to_vec_pretty(&payload).expect("output JSON"))
        .expect("write outside every timed interval");
}

#[path = "native_cost/representative.rs"]
mod representative;
