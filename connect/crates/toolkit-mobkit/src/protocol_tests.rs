//! Private admission-protocol and SQLite tests with a controlled host fixture.
//! They do not establish runtime attachment, peer isolation or worker launch.
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_core::{
    auth::{PrincipalKind, PrincipalRef, TrustDomainId},
    authorization::WorkAuthorizationContext,
    connection::RealmId,
};
fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        value,
        TrustDomainId::new("toolkit-fixture").unwrap(),
    )
    .unwrap()
}
fn dest(value: &str, requester: &PrincipalRef) -> NativeWorkTarget {
    native_identity::target(value, requester)
}
mod native_identity;

use crate::*;
use async_trait::async_trait;
use meerkat_core::{
    AgentToolDispatcher, SessionId, ToolCallView, ToolDispatchContext, ToolError,
    ToolMutationClass, interaction::InteractionId, ops::ToolDispatchOutcome,
};
use meerkat_toolkit::{authority::*, request::*, setup::*};
use serde_json::{Value, value::RawValue};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use toolkit_package::{ContentDigest, Id, Revision};
use toolkit_store::*;

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn rev(value: &str) -> Revision {
    Revision::new(value).unwrap()
}
fn context_reference() -> ContextReference {
    ContextReference {
        owner: id("context-owner"),
        id: id("original-context"),
        revision: rev("1"),
        content_digest: ContentDigest::of_bytes(b"original context"),
    }
}
fn input() -> Value {
    serde_json::json!({"request": "I vaguely remember Paper-something. Connect it; keep it private.\nPreserve this line.",
        "user_context": ["Original constraint", context_reference()],
        "caller_observations": ["I have no suitable integration yet."]})
}
struct References {
    allowed: AtomicBool,
    checks: AtomicUsize,
    deny_at: AtomicUsize,
    encoding_failure: AtomicUsize,
}
impl ContextReferenceOwner for References {
    fn verify_access(
        &self,
        reference: &ContextReference,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        authority.check_context(context)?;
        let check = self.checks.fetch_add(1, Ordering::SeqCst) + 1;
        match self.encoding_failure.load(Ordering::SeqCst) {
            1 => return Err(RequestError::Encoding(
                <serde_json::Error as serde::ser::Error>::custom("private reference encoding detail"),
            )),
            2 => return Err(RequestError::Authority(SetupContextError::InvalidEncoding)),
            _ => {}
        }
        if self.allowed.load(Ordering::SeqCst)
            && check < self.deny_at.load(Ordering::SeqCst)
            && *reference == context_reference()
        {
            Ok(())
        } else {
            Err(RequestError::ContextUnavailable)
        }
    }
}
struct Host {
    session: SessionId,
    interaction: InteractionId,
    evidence: SetupRequestEvidence,
    authorization: WorkAuthorizationContext,
    target: Id,
    audience: Id,
    calls: AtomicUsize,
    mode: AtomicUsize,
}
impl Host {
    fn admission(&self, call_id: &str) -> HostToolAdmission {
        let correlation =
            ContentDigest::of_bytes(format!("{}:{call_id}", self.audience).as_bytes()).to_string();
        HostToolAdmission {
            host: HostSetupRequest {
                setup_helper: None,
                target: self.target.clone(),
                correlation: id(&correlation),
                parent_operation: Some(id("original-task")),
                continuation: TaskContinuation { native_original: None,
                    owner: id("runtime"),
                    task: id("original-task"),
                    revision: rev("2"),
                    delivery_key: id(&format!("delivery-{correlation}")),
                    audience: [self.audience.clone()].into(),
                },
                environment_revision: rev("environment-1"),
                limits: SetupLimits {
                    deadline_ms: 900,
                    max_workers: 3,
                    max_effect_attempts: 5,
                },
            },
            evidence: self.evidence.clone(),
            context: HostAdmissionContext {
                tenant: id("tenant-a"),
                deployment: id("deployment-a"),
                realm: RealmId::parse("core-realm").unwrap(),
                target: self.evidence.association().candidate().target.clone(),
                policy_revision: rev("policy-1"),
                now_ms: 200,
            },
        }
    }
    fn context(&self) -> ToolDispatchContext {
        ToolDispatchContext::default()
            .with_runtime_identity(self.session.clone(), Some(self.interaction))
            .with_work_authorization(Some(self.authorization.clone()))
    }
}
#[async_trait]
impl SetupToolHost for Host {
    async fn admit_invocation(
        &self,
        call_id: &str,
        context: &ToolDispatchContext,
    ) -> Result<HostToolAdmission, ToolAdmissionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // This fixture holds an authenticated session/task. Public IDs alone
        // could not perform this authentication in a production host adapter.
        if context.origin_session_id() != Some(&self.session)
            || context.interaction_lineage_id() != Some(self.interaction)
        {
            return Err(ToolAdmissionError::Denied);
        }
        match self.mode.load(Ordering::SeqCst) {
            1 => return Err(ToolAdmissionError::Denied),
            2 => return Err(ToolAdmissionError::Unavailable),
            3 => {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            _ => {}
        }
        let mut result = self.admission(call_id);
        match self.mode.load(Ordering::SeqCst) {
            4 => result.context.policy_revision = rev("policy-2"),
            5 => {
                result
                    .host
                    .continuation
                    .audience
                    .insert(id("monitoring-channel"));
            }
            6 => {
                result.context.tenant = id("other-tenant");
            }
            7 => {
                result.context.now_ms = 1001;
            }
            8 => {
                // Setup deadline elapsed, original authority remains current.
                result.context.now_ms = 950;
            }
            _ => {}
        };
        Ok(result)
    }
}
struct Fixture {
    directory: tempfile::TempDir,
    store: Arc<SqliteStore>,
    host: Arc<Host>,
    references: Arc<References>,
}
impl Fixture {
    fn new() -> Self {
        Self::for_person("person-a")
    }
    fn for_person(person: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(directory.path().join("toolkit.sqlite")).unwrap());
        let target = id("agent-a");
        let tenant = id("tenant-a");
        let deployment = id("deployment-a");
        let policy = rev("policy-1");
        let association = native_identity::association(
            principal(person),
            dest(target.as_str(), &principal(person)),
            &format!("original-{person}"),
            &format!("event-{person}"),
        );
        let authorization = native_identity::current(&association);
        let evidence = SetupRequestEvidence::new(
            association,
            SetupProposalConstraints {
                tenant,
                deployment,
                policy_revision: policy,
                issued_at_ms: 100,
                expires_at_ms: 1000,
                ceiling: Ceiling {
                    actions: [ActionPermission {
                        action: Action::FindAndInstall,
                        resource: target.clone(),
                    }]
                    .into(),
                    services: Default::default(),
                    frontends: Default::default(),
                    output_audiences: [id(person)].into(),
                },
            },
        )
        .unwrap();
        Self {
            directory,
            store,
            host: Arc::new(Host {
                session: SessionId::new(),
                interaction: InteractionId::new(),
                evidence,
                authorization,
                target,
                audience: id(person),
                calls: 0.into(),
                mode: 0.into(),
            }),
            references: Arc::new(References {
                allowed: true.into(),
                checks: 0.into(),
                deny_at: usize::MAX.into(),
                encoding_failure: 0.into(),
            }),
        }
    }
    fn scope(&self) -> SetupScope {
        SetupScope::new(id("tenant-a"), id("deployment-a"))
    }
    // Successful SQLite setup uses 2 seconds; timeout controls opt into 50ms.
    fn tool_with_store(&self, store: Arc<SqliteStore>) -> FindAndInstallTool {
        FindAndInstallTool::new(
            store,
            self.scope(),
            self.host.clone(),
            self.references.clone(),
            Duration::from_secs(2),
        )
        .unwrap()
    }
    fn tool(&self) -> FindAndInstallTool {
        self.tool_with_store(self.store.clone())
    }
    async fn records(&self) -> Vec<StoredRecord> {
        self.store
            .list_records(
                &StoreScope {
                    realm: "tenant-a".into(),
                    owner: "toolkit/acquisition/deployment-a".into(),
                },
                None,
                100,
            )
            .await
            .unwrap()
    }
    async fn load(&self, operation: &str) -> SetupSnapshot {
        let a = self.host.admission("call-1");
        SetupStore::new(self.store.as_ref(), self.scope())
            .unwrap()
            .load(
                &id(operation),
                &a.evidence,
                &SetupUseContext {
                    tenant: &a.context.tenant,
                    deployment: &a.context.deployment,
                    realm: &a.context.realm,
                    target: &a.context.target,
                    authorization: &self.host.authorization,
                    policy_revision: &a.context.policy_revision,
                    now_ms: 200,
                },
                self.references.as_ref(),
            )
            .await
            .unwrap()
    }
}
async fn invoke(
    tool: &FindAndInstallTool,
    host: &Host,
    call_id: &str,
    args: Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw = RawValue::from_string(args.to_string()).unwrap();
    tool.dispatch_protocol_fixture(
        ToolCallView {
            id: call_id,
            name: FIND_AND_INSTALL,
            args: &raw,
        },
        &host.context(),
    )
    .await
}
fn reply(outcome: &ToolDispatchOutcome) -> Value {
    assert!(!outcome.result.is_error);
    assert!(outcome.async_ops.is_empty());
    assert!(outcome.session_effects.is_empty());
    serde_json::from_str(&outcome.result.text_content()).unwrap()
}

#[tokio::test]
async fn persists_original_input_and_one_handoff_before_inline_response() {
    let f = Fixture::new();
    let expected = input();
    let outcome = invoke(&f.tool(), &f.host, "call-1", expected.clone())
        .await
        .unwrap();
    assert_eq!(outcome.result.tool_use_id, "call-1");
    let result = reply(&outcome);
    assert_eq!(result["phase"], "investigating");
    assert_eq!(result["revision"], 1);
    assert_eq!(result["created"], true);
    assert!(!outcome.result.text_content().contains("Paper"));
    let saved = f.load(result["operation"].as_str().unwrap()).await;
    assert_eq!(
        serde_json::to_value(saved.request().request()).unwrap(),
        expected
    );
    assert_eq!(
        saved.request().host().continuation.audience,
        [id("person-a")].into()
    );
    assert!(saved.request().constraints().ceiling.services.is_empty());
    assert!(saved.request().constraints().ceiling.frontends.is_empty());
    let effects = f.store.list_effects(saved.key(), None, 10).await.unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].intent.kind, "work_admission_requested");
    assert_eq!(effects[0].status, EffectStatus::Pending);
    assert_eq!(effects[0].attempt, 0);
    assert_eq!(
        f.store.read_events(saved.key(), 0, 10).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn lost_reply_reopens_same_operation_and_changed_content_conflicts() {
    let f = Fixture::new();
    let original = reply(&invoke(&f.tool(), &f.host, "call-1", input()).await.unwrap());
    let reopened = Arc::new(SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap());
    let tool = f.tool_with_store(reopened);
    let repeated = reply(&invoke(&tool, &f.host, "call-1", input()).await.unwrap());
    assert_eq!(original["operation"], repeated["operation"]);
    assert_eq!(repeated["created"], false);
    assert_eq!(repeated["revision"], 1);
    let mut changed = input();
    changed["request"] = serde_json::json!("different secret request");
    let error = invoke(&tool, &f.host, "call-1", changed).await.unwrap_err();
    assert!(matches!(error, ToolError::InvalidArguments { .. }));
    assert!(!error.to_transcript_content().contains("secret"));
    assert_eq!(f.records().await.len(), 1);
    let new = reply(&invoke(&tool, &f.host, "call-2", input()).await.unwrap());
    assert_ne!(original["operation"], new["operation"]);
    assert_eq!(f.records().await.len(), 2);
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn concurrent_same_invocation_uses_one_record_and_one_handoff() {
    let f = Fixture::new();
    // This is a concurrent success test; deliberate timeout tests keep 50ms.
    let tool = Arc::new(
        FindAndInstallTool::new(
            f.store.clone(),
            f.scope(),
            f.host.clone(),
            f.references.clone(),
            Duration::from_secs(2),
        )
        .unwrap(),
    );
    let mut workers = Vec::new();
    for _ in 0..12 {
        let tool = tool.clone();
        let host = f.host.clone();
        workers.push(tokio::spawn(async move {
            invoke(tool.as_ref(), &host, "call-1", input()).await
        }));
    }
    let mut outcomes = Vec::new();
    for worker in workers {
        outcomes.push(worker.await);
    }
    // Join every worker before a failed result can end the test.
    let results: Vec<_> = outcomes
        .into_iter()
        .map(|outcome| reply(&outcome.unwrap().unwrap()))
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|result| result["created"] == true)
            .count(),
        1
    );
    assert!(
        results
            .iter()
            .all(|result| result["operation"] == results[0]["operation"])
    );
    let records = f.records().await;
    assert_eq!(records.len(), 1);
    assert_eq!(
        f.store
            .list_effects(&records[0].key, None, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 12);
}

#[tokio::test]
async fn content_schema_rejects_authority_routing_and_nested_smuggling() {
    let f = Fixture::new();
    for key in [
        "tenant",
        "authority",
        "target",
        "correlation",
        "continuation",
        "deployment",
        "package",
        "backend",
        "budget",
        "fan_out_group",
    ] {
        let mut value = input();
        value[key] = serde_json::json!("private-credential-sentinel");
        let error = invoke(&f.tool(), &f.host, "call-1", value)
            .await
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArguments { .. }));
        assert!(
            !error
                .to_transcript_content()
                .contains("private-credential-sentinel")
        );
    }
    let mut value = input();
    value["user_context"][1]["authority"] = serde_json::json!("private-credential-sentinel");
    assert!(matches!(
        invoke(&f.tool(), &f.host, "call-1", value).await,
        Err(ToolError::InvalidArguments { .. })
    ));
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 0);
    assert!(f.records().await.is_empty());
}

#[tokio::test]
async fn plain_or_missing_runtime_context_never_reaches_host_or_store() {
    let f = Fixture::new();
    let tool = f.tool();
    let raw = RawValue::from_string(input().to_string()).unwrap();
    let call = ToolCallView {
        id: "call-1",
        name: FIND_AND_INSTALL,
        args: &raw,
    };
    assert!(matches!(
        tool.dispatch(call).await,
        Err(ToolError::AccessDenied { .. })
    ));
    for context in [
        ToolDispatchContext::default(),
        ToolDispatchContext::default().with_runtime_identity(f.host.session.clone(), None),
        ToolDispatchContext::default()
            .with_runtime_identity(f.host.session.clone(), Some(f.host.interaction)),
    ] {
        assert!(matches!(
            tool.dispatch_protocol_fixture(call, &context).await,
            Err(ToolError::AccessDenied { .. })
        ));
    }
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 0);
    let wrong = ToolDispatchContext::default()
        .with_runtime_identity(SessionId::new(), Some(InteractionId::new()))
        .with_work_authorization(Some(f.host.authorization.clone()))
        .with_turn_metadata([("authority".into(), serde_json::json!("trusted"))].into());
    assert!(matches!(
        tool.dispatch_protocol_fixture(call, &wrong).await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 1);
    assert!(f.records().await.is_empty());
}

#[tokio::test]
async fn current_host_and_reference_permissions_are_rechecked_on_replay() {
    let f = Fixture::new();
    let tool = f.tool();
    invoke(&tool, &f.host, "call-1", input()).await.unwrap();
    for mode in [1, 4, 5, 6, 7] {
        f.host.mode.store(mode, Ordering::SeqCst);
        assert!(matches!(
            invoke(&tool, &f.host, "call-1", input()).await,
            Err(ToolError::AccessDenied { .. })
        ));
    }
    f.host.mode.store(0, Ordering::SeqCst);
    f.references.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        invoke(&tool, &f.host, "call-1", input()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(f.records().await.len(), 1);
    let records = f.records().await;
    assert_eq!(
        f.store
            .read_events(&records[0].key, 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn reference_revocation_between_validation_and_admission_writes_nothing() {
    let f = Fixture::new();
    f.references.deny_at.store(2, Ordering::SeqCst);
    assert!(matches!(
        invoke(&f.tool(), &f.host, "call-1", input()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(f.references.checks.load(Ordering::SeqCst), 2);
    assert!(f.records().await.is_empty());
}

#[tokio::test]
async fn host_unavailable_and_timeout_are_redacted_and_leave_no_setup() {
    let f = Fixture::new();
    let tool = FindAndInstallTool::new(
        f.store.clone(),
        f.scope(),
        f.host.clone(),
        f.references.clone(),
        Duration::from_millis(50),
    )
    .unwrap();
    for mode in [2, 3] {
        f.host.mode.store(mode, Ordering::SeqCst);
        let error = invoke(&tool, &f.host, "call-1", input())
            .await
            .unwrap_err();
        assert!(matches!(error, ToolError::Unavailable { .. }));
        assert!(!error.to_transcript_content().contains("Paper"));
    }
    assert!(f.records().await.is_empty());
}

#[tokio::test]
async fn reference_encoding_unavailable_is_redacted_and_preserves_first_and_replay_storage() {
    // Existing reference-owner failures exercise the actual request projection
    // and SQLite bridge. They are not evidence of a native serialization fault.
    for mode in [1, 2] {
        let f = Fixture::new();
        let tool = f.tool();
        f.references.encoding_failure.store(mode, Ordering::SeqCst);
        let error = invoke(&tool, &f.host, "call-1", input()).await.unwrap_err();
        assert!(matches!(&error, ToolError::Unavailable {
            name, reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
        } if name == FIND_AND_INSTALL));
        assert_eq!(error.to_transcript_content(), unavailable().to_transcript_content());
        assert!(!error.to_transcript_content().contains("private reference encoding detail"));
        assert!(!error.to_transcript_content().contains("Paper"));
        assert_eq!(f.references.checks.load(Ordering::SeqCst), 1);
        assert!(f.records().await.is_empty());

        f.references.encoding_failure.store(0, Ordering::SeqCst);
        let first = reply(&invoke(&tool, &f.host, "call-1", input()).await.unwrap());
        assert_eq!(first["created"], true);
        let records = f.records().await;
        assert_eq!(records.len(), 1);
        let events = f.store.read_events(&records[0].key, 0, 100).await.unwrap();
        let effects = f.store.list_effects(&records[0].key, None, 20).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(effects.len(), 1);
        let checks = f.references.checks.load(Ordering::SeqCst);

        f.references.encoding_failure.store(mode, Ordering::SeqCst);
        let error = invoke(&tool, &f.host, "call-1", input()).await.unwrap_err();
        assert!(matches!(&error, ToolError::Unavailable {
            name, reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
        } if name == FIND_AND_INSTALL));
        assert_eq!(error.to_transcript_content(), unavailable().to_transcript_content());
        assert!(!error.to_transcript_content().contains("private reference encoding detail"));
        assert!(!error.to_transcript_content().contains("Paper"));
        assert_eq!(f.references.checks.load(Ordering::SeqCst), checks + 1);
        assert_eq!(f.records().await, records);
        assert_eq!(f.store.read_events(&records[0].key, 0, 100).await.unwrap(), events);
        assert_eq!(f.store.list_effects(&records[0].key, None, 20).await.unwrap(), effects);

        f.references.encoding_failure.store(0, Ordering::SeqCst);
        let replay = reply(&invoke(&tool, &f.host, "call-1", input()).await.unwrap());
        assert_eq!(replay["created"], false);
        assert_eq!(replay["operation"], first["operation"]);
        assert_eq!(f.records().await, records);
        assert_eq!(f.store.read_events(&records[0].key, 0, 100).await.unwrap(), events);
        assert_eq!(f.store.list_effects(&records[0].key, None, 20).await.unwrap(), effects);
    }
}

#[tokio::test]
async fn limits_invalid_configuration_and_wrong_scope_fail_before_mutation() {
    let f = Fixture::new();
    let tool = f.tool();
    for value in [
        serde_json::json!({"request": "   "}),
        serde_json::json!({"request": "x", "caller_observations": vec!["x";257]}),
        serde_json::json!({"request": "x".repeat(1024*1024)}),
    ] {
        assert!(matches!(
            invoke(&tool, &f.host, "call-1", value).await,
            Err(ToolError::InvalidArguments { .. })
        ));
    }
    for call_id in [String::new(), "x".repeat(513)] {
        assert!(matches!(
            invoke(&tool, &f.host, &call_id, input()).await,
            Err(ToolError::InvalidArguments { .. })
        ));
    }
    for timeout in [Duration::ZERO, Duration::from_secs(61)] {
        assert!(
            FindAndInstallTool::new(
                f.store.clone(),
                f.scope(),
                f.host.clone(),
                f.references.clone(),
                timeout
            )
            .is_err()
        );
    }
    let wrong_scope = FindAndInstallTool::new(
        f.store.clone(),
        SetupScope::new(id("other-tenant"), id("deployment-a")),
        f.host.clone(),
        f.references.clone(),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(matches!(
        invoke(&wrong_scope, &f.host, "call-1", input()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert!(f.records().await.is_empty());
}

#[tokio::test]
async fn leaf_catalog_is_explicit_mutating_and_does_not_dispatch_unknown_tools() {
    let f = Fixture::new();
    let tool = f.tool();
    assert_eq!(tool.tools().len(), 1);
    assert_eq!(tool.tools()[0].name, FIND_AND_INSTALL);
    assert!(tool.tool_catalog_capabilities().exact_catalog);
    assert!(
        !tool
            .tool_catalog_capabilities()
            .may_require_catalog_control_plane
    );
    assert_eq!(tool.tool_catalog().len(), 1);
    assert_eq!(
        tool.tool_mutation_class(FIND_AND_INSTALL),
        ToolMutationClass::Mutating
    );
    assert_eq!(
        tool.tool_mutation_class("unknown"),
        ToolMutationClass::Unknown
    );
    let raw = RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView {
        id: "1",
        name: "unknown",
        args: &raw,
    };
    assert!(matches!(
        tool.dispatch(call).await,
        Err(ToolError::NotFound { .. })
    ));
    assert!(matches!(
        tool.dispatch_protocol_fixture(call, &f.host.context())
            .await,
        Err(ToolError::NotFound { .. })
    ));
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 0);
    assert!(f.records().await.is_empty());
}

#[tokio::test]
async fn authenticated_principals_do_not_share_correlation_or_read_each_others_operation() {
    let a = Fixture::new();
    let b = Fixture::for_person("person-b");
    let first = reply(&invoke(&a.tool(), &a.host, "call-1", input()).await.unwrap());
    let b_tool = FindAndInstallTool::new(
        a.store.clone(),
        a.scope(),
        b.host.clone(),
        b.references.clone(),
        Duration::from_secs(2),
    )
    .unwrap();
    let second = reply(&invoke(&b_tool, &b.host, "call-1", input()).await.unwrap());
    assert_ne!(first["operation"], second["operation"]);
    let admission = b.host.admission("call-1");
    let context = SetupUseContext {
        tenant: &admission.context.tenant,
        deployment: &admission.context.deployment,
        realm: &admission.context.realm,
        target: &admission.context.target,
        authorization: &b.host.authorization,
        policy_revision: &admission.context.policy_revision,
        now_ms: 200,
    };
    let bridge = SetupStore::new(a.store.as_ref(), a.scope()).unwrap();
    assert!(
        bridge
            .load(
                &id(first["operation"].as_str().unwrap()),
                &admission.evidence,
                &context,
                b.references.as_ref()
            )
            .await
            .is_err()
    );
    let saved = bridge
        .load(
            &id(second["operation"].as_str().unwrap()),
            &admission.evidence,
            &context,
            b.references.as_ref(),
        )
        .await
        .unwrap();
    assert_eq!(
        saved
            .request()
            .evidence()
            .association()
            .candidate()
            .requester,
        principal("person-b")
    );
    assert_eq!(
        saved.request().host().continuation.audience,
        [id("person-b")].into()
    );
    assert_eq!(a.records().await.len(), 2);
}

#[tokio::test]
async fn terminal_replay_reports_generated_phase_without_new_work_or_readiness_claim() {
    let f = Fixture::new();
    let tool = f.tool();
    let first = reply(&invoke(&tool, &f.host, "call-1", input()).await.unwrap());
    let operation = id(first["operation"].as_str().unwrap());
    let admission = f.host.admission("call-1");
    let context = SetupUseContext {
        tenant: &admission.context.tenant,
        deployment: &admission.context.deployment,
        realm: &admission.context.realm,
        target: &admission.context.target,
        authorization: &f.host.authorization,
        policy_revision: &admission.context.policy_revision,
        now_ms: 200,
    };
    let bridge = SetupStore::new(f.store.as_ref(), f.scope()).unwrap();
    let cancelled = bridge
        .cancel(
            &operation,
            context_reference(),
            SetupAccess {
                evidence: &admission.evidence,
                context: &context,
                references: f.references.as_ref(),
            },
        )
        .await
        .unwrap();
    let repeat = reply(&invoke(&tool, &f.host, "call-1", input()).await.unwrap());
    assert_eq!(repeat["operation"], first["operation"]);
    assert_eq!(repeat["phase"], "cancelled");
    assert_eq!(repeat["revision"], cancelled.revision);
    assert_eq!(repeat["created"], false);
    let record = &f.records().await[0];
    let effects = f.store.list_effects(&record.key, None, 20).await.unwrap();
    assert_eq!(
        effects
            .iter()
            .filter(|effect| effect.intent.kind == "work_admission_requested")
            .count(),
        1
    );
    assert_eq!(
        effects
            .iter()
            .filter(|effect| effect.intent.kind == "continuation_requested")
            .count(),
        1
    );
    assert!(effects.iter().all(|effect| effect.attempt == 0));
}

#[tokio::test]
async fn storage_timeout_can_commit_late_and_exact_retry_recovers_one_operation() {
    let f = Fixture::new();
    let tool = FindAndInstallTool::new(
        f.store.clone(),
        f.scope(),
        f.host.clone(),
        f.references.clone(),
        Duration::from_millis(50),
    )
    .unwrap();
    let lock = rusqlite::Connection::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let error = invoke(&tool, &f.host, "call-1", input())
        .await
        .unwrap_err();
    assert!(matches!(error, ToolError::Unavailable { .. }));
    // SQLite's started blocking worker is not cancelled with its async caller.
    // Release it before replay; either commit ordering must converge to one row.
    lock.execute_batch("COMMIT").unwrap();
    let retry_tool = FindAndInstallTool::new(
        f.store.clone(),
        f.scope(),
        f.host.clone(),
        f.references.clone(),
        Duration::from_secs(2),
    )
    .unwrap();
    let result = reply(
        &invoke(&retry_tool, &f.host, "call-1", input())
            .await
            .unwrap(),
    );
    assert_eq!(result["phase"], "investigating");
    let records = f.records().await;
    assert_eq!(records.len(), 1);
    assert_eq!(
        f.store
            .list_effects(&records[0].key, None, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.store
            .read_events(&records[0].key, 0, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn exact_lost_reply_recovery_after_deadline_cannot_admit_new_work() {
    let f = Fixture::new();
    let first = reply(&invoke(&f.tool(), &f.host, "call-1", input()).await.unwrap());
    let records_before = f.records().await;
    let effects_before = f
        .store
        .list_effects(&records_before[0].key, None, 10)
        .await
        .unwrap();
    let events_before = f
        .store
        .read_events(&records_before[0].key, 0, 100)
        .await
        .unwrap();
    f.host.mode.store(8, Ordering::SeqCst);
    let reopened = Arc::new(SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap());
    let recovered = reply(
        &invoke(&f.tool_with_store(reopened), &f.host, "call-1", input())
            .await
            .unwrap(),
    );
    assert_eq!(recovered["operation"], first["operation"]);
    assert_eq!(recovered["revision"], first["revision"]);
    assert_eq!(recovered["phase"], first["phase"]);
    assert_eq!(recovered["created"], false);
    assert!(matches!(
        invoke(&f.tool(), &f.host, "never-admitted", input()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    let mut changed = input();
    changed["request"] = serde_json::json!("Changed meaning after deadline");
    assert!(matches!(
        invoke(&f.tool(), &f.host, "call-1", changed).await,
        Err(ToolError::InvalidArguments { .. })
    ));
    assert_eq!(f.records().await, records_before);
    assert_eq!(
        f.store
            .list_effects(&records_before[0].key, None, 10)
            .await
            .unwrap(),
        effects_before
    );
    assert_eq!(
        f.store
            .read_events(&records_before[0].key, 0, 100)
            .await
            .unwrap(),
        events_before
    );

    f.references.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        invoke(&f.tool(), &f.host, "call-1", input()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    f.references.allowed.store(true, Ordering::SeqCst);
    f.host.mode.store(7, Ordering::SeqCst);
    assert!(matches!(
        invoke(&f.tool(), &f.host, "call-1", input()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    f.host.mode.store(4, Ordering::SeqCst);
    assert!(matches!(
        invoke(&f.tool(), &f.host, "call-1", input()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    f.host.mode.store(1, Ordering::SeqCst);
    assert!(matches!(
        invoke(&f.tool(), &f.host, "call-1", input()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(f.records().await, records_before);
}

#[tokio::test]
async fn direct_bridge_exact_recovery_preserves_deadline_and_observation_access() {
    let f = Fixture::new();
    let admission = f.host.admission("call-1");
    let host_context = |now_ms| SetupUseContext {
        tenant: &admission.context.tenant,
        deployment: &admission.context.deployment,
        realm: &admission.context.realm,
        target: &admission.context.target,
        authorization: &f.host.authorization,
        policy_revision: &admission.context.policy_revision,
        now_ms,
    };
    let original = validate_request(
        serde_json::from_value(input()).unwrap(),
        admission.host.clone(),
        &admission.evidence,
        &host_context(200),
        f.references.as_ref(),
    )
    .unwrap();
    let bridge = SetupStore::new(f.store.as_ref(), f.scope()).unwrap();
    let first = bridge
        .admit(
            &original,
            &admission.evidence,
            &host_context(200),
            f.references.as_ref(),
        )
        .await
        .unwrap();
    let recovered = bridge
        .admit(
            &original,
            &admission.evidence,
            &host_context(950),
            f.references.as_ref(),
        )
        .await
        .unwrap();
    assert!(!recovered.created);
    assert_eq!(recovered.snapshot.key(), first.snapshot.key());
    assert_eq!(recovered.snapshot.request().host().limits.deadline_ms, 900);
    assert!(
        original
            .revalidate(
                &admission.evidence,
                &host_context(950),
                f.references.as_ref()
            )
            .is_err()
    );
    let other = SqliteStore::open(f.directory.path().join("empty.sqlite")).unwrap();
    let empty = SetupStore::new(&other, f.scope()).unwrap();
    assert!(matches!(
        empty
            .admit(
                &original,
                &admission.evidence,
                &host_context(950),
                f.references.as_ref()
            )
            .await,
        Err(SetupError::Request(RequestError::InvalidLimits))
    ));
    assert!(
        other
            .list_records(&first.snapshot.key().scope(), None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    f.references.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        bridge
            .admit(
                &original,
                &admission.evidence,
                &host_context(950),
                f.references.as_ref()
            )
            .await,
        Err(SetupError::Request(RequestError::ContextUnavailable))
    ));
}

#[tokio::test]
async fn production_dispatch_refuses_before_host_references_and_storage() {
    let f = Fixture::new();
    let tool = f.tool();
    let raw = RawValue::from_string(input().to_string()).unwrap();
    let result = tool
        .dispatch_with_context(
            ToolCallView {
                id: "native-unavailable",
                name: FIND_AND_INSTALL,
                args: &raw,
            },
            &f.host.context(),
        )
        .await;
    assert!(matches!(
        result,
        Err(ToolError::Unavailable { name, reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable })
            if name == FIND_AND_INSTALL
    ));
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.references.checks.load(Ordering::SeqCst), 0);
    assert!(f.records().await.is_empty());
}

mod native_custody;


#[tokio::test]
async fn installed_admission_clock_race_retains_winner_without_accepting_changed_input() {
    let f = Fixture::new();
    let first = f.host.admission("clock-race");
    let mut other = f.host.admission("clock-race");
    let mut constraints = other.evidence.constraints().clone();
    constraints.issued_at_ms = 150;
    constraints.expires_at_ms = 1050;
    other.evidence = SetupRequestEvidence::new(other.evidence.association().clone(), constraints).unwrap();
    other.host.limits.deadline_ms = 950;
    assert!(same_installed_admission(&first, &other));
    let bridge = SetupStore::new_conditional(f.store.as_ref(), SetupScope::new(id("tenant-a"), id("deployment-a"))).unwrap();
    let raw: FindAndInstallRequest = serde_json::from_value(input()).unwrap();
    let winner = bridge.admit_or_recover(raw.clone(), first.host.clone(), &first.evidence,
        &first.context.borrowed(&f.host.authorization), f.references.as_ref()).await.unwrap();
    assert!(matches!(bridge.admit_or_recover(raw.clone(), other.host.clone(), &other.evidence,
        &other.context.borrowed(&f.host.authorization), f.references.as_ref()).await,
        Err(SetupError::Store(StoreError::AdmissionConflict))));
    let saved = bridge.invocation_terms(other.evidence.association(), &other.host.correlation,
        &other.context.borrowed(&f.host.authorization)).await.unwrap().unwrap();
    other.host = saved.host;
    other.evidence = saved.evidence;
    assert!(same_installed_admission(&first, &other));
    let replay = bridge.admit_or_recover(raw.clone(), other.host.clone(), &other.evidence,
        &other.context.borrowed(&f.host.authorization), f.references.as_ref()).await.unwrap();
    assert!(!replay.created);
    assert_eq!(winner.snapshot.key(), replay.snapshot.key());
    assert_eq!(replay.snapshot.request().host().limits.deadline_ms, 900);
    let mut changed = raw;
    changed.caller_observations.push("Different exact content".into());
    assert!(matches!(bridge.admit_or_recover(changed, other.host.clone(), &other.evidence,
        &other.context.borrowed(&f.host.authorization), f.references.as_ref()).await,
        Err(SetupError::Store(StoreError::AdmissionConflict))));
    other.host.continuation.audience.insert(id("foreign-audience"));
    assert!(!same_installed_admission(&first, &other));
}


struct FirstAdmissionClockRace {
    original: Arc<Host>,
    calls: AtomicUsize,
}
#[async_trait]
impl SetupToolHost for FirstAdmissionClockRace {
    async fn admit_invocation(&self, call_id: &str, context: &ToolDispatchContext)
        -> Result<HostToolAdmission, ToolAdmissionError>
    {
        let mut admission = self.original.admit_invocation(call_id, context).await?;
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let mut terms = admission.evidence.constraints().clone();
            terms.issued_at_ms = 150;
            terms.expires_at_ms = 1050;
            admission.evidence = SetupRequestEvidence::new(admission.evidence.association().clone(), terms)
                .map_err(|_| ToolAdmissionError::Unavailable)?;
            admission.host.limits.deadline_ms = 950;
        }
        Ok(admission)
    }
}

// Runs the actual conditional admission retry after an already won clock race.
// The private protocol fixture selects standalone admission dependencies. No
// acquisition driver is installed, so this proves admission recovery, not launch.
#[tokio::test]
async fn installed_tool_retries_clock_conflict_once_with_exact_raw_input() {
    let f = Fixture::new();
    let canonical = f.host.admission("dispatch-clock-race");
    let bridge = SetupStore::new_conditional(f.store.as_ref(), SetupScope::new(id("tenant-a"), id("deployment-a"))).unwrap();
    let raw: FindAndInstallRequest = serde_json::from_value(input()).unwrap();
    let winner = bridge.admit_or_recover(raw, canonical.host.clone(), &canonical.evidence,
        &canonical.context.borrowed(&f.host.authorization), f.references.as_ref()).await.unwrap();
    let held = f.store.get(winner.snapshot.key()).await.unwrap().unwrap();
    let host = Arc::new(FirstAdmissionClockRace { original: f.host.clone(), calls: AtomicUsize::new(0) });
    let mut tool = FindAndInstallTool::new(f.store.clone(), SetupScope::new(id("tenant-a"), id("deployment-a")),
        host.clone(), f.references.clone(), Duration::from_secs(1)).unwrap();
    match &mut tool.admission {
        FindAndInstallAdmission::Direct(dependencies) => {
            dependencies.conditional_store = Some(f.store.clone());
            assert!(Arc::ptr_eq(dependencies.conditional_store.as_ref().unwrap(), &f.store));
        }
        #[cfg(all(feature = "native-host", unix))]
        FindAndInstallAdmission::Installed(_) => panic!("the protocol fixture must retain direct admission dependencies"),
    }
    #[cfg(all(feature = "native-host", unix))]
    assert!(tool.admission_owner().unwrap().acquisition.is_none());
    let args = RawValue::from_string(input().to_string()).unwrap();
    let context = f.host.context();
    let call = ToolCallView { id: "dispatch-clock-race", name: FIND_AND_INSTALL, args: args.as_ref() };
    assert!(matches!(tool.dispatch_with_context(call, &context).await,
        Err(ToolError::Unavailable { .. })), "standalone dependencies cannot enable native execution");
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.store.get(winner.snapshot.key()).await.unwrap().unwrap(), held);
    let outcome = tool.dispatch_protocol_fixture(call, &context).await.unwrap();
    let observed = reply(&outcome);
    assert_eq!(observed["operation"], winner.snapshot.key().id);
    assert_eq!(observed["created"], false);
    assert!(observed.get("worker").is_none(), "direct admission must not launch a native worker");
    assert_eq!(host.calls.load(Ordering::SeqCst), 2, "actual tool requested canonical winning terms exactly once");
    assert_eq!(f.store.get(winner.snapshot.key()).await.unwrap().unwrap(), held);
    host.calls.store(0, Ordering::SeqCst);
    let mut changed = input();
    changed["request"] = Value::String("Changed original content".into());
    let args = RawValue::from_string(changed.to_string()).unwrap();
    let call = ToolCallView { id: "dispatch-clock-race", name: FIND_AND_INSTALL, args: args.as_ref() };
    assert!(matches!(tool.dispatch_protocol_fixture(call, &context).await, Err(ToolError::InvalidArguments { .. })));
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.store.get(winner.snapshot.key()).await.unwrap().unwrap(), held);
}


#[cfg(feature = "native-host")]
#[tokio::test]
async fn helper_review_lifetime_binds_exact_canonical_snapshot_and_refuses_changed_or_terminal() {
    use crate::{original_ingress::helper_originals::HelperOperation,
        private_originals::{PrivateOriginalError, setup_helpers::bind_snapshot_deadline_fixture}};
    use std::sync::OnceLock;
    let f = Fixture::new();
    let admission = f.host.admission("call-1");
    let context = SetupUseContext {
        tenant: &admission.context.tenant, deployment: &admission.context.deployment,
        realm: &admission.context.realm, target: &admission.context.target,
        authorization: &f.host.authorization, policy_revision: &admission.context.policy_revision,
        now_ms: 200,
    };
    let request = validate_request(serde_json::from_value(input()).unwrap(), admission.host.clone(),
        &admission.evidence, &context, f.references.as_ref()).unwrap();
    let bridge = SetupStore::new(f.store.as_ref(), f.scope()).unwrap();
    let snapshot = bridge.admit(&request, &admission.evidence, &context, f.references.as_ref())
        .await.unwrap().snapshot;
    let operation = HelperOperation {
        acquisition: snapshot.key().clone(), effect: "work-admission".into(),
        input_digest: ContentDigest::of_bytes(&toolkit_package::canonical_json(request.request()).unwrap()),
    };
    let held = OnceLock::new();
    bind_snapshot_deadline_fixture(&held, &operation, &snapshot).unwrap();
    bind_snapshot_deadline_fixture(&held, &operation, &snapshot).unwrap();
    assert_eq!(held.get(), Some(&900));
    // These are real independently validated canonical rows, not modified
    // snapshots or model-provided deadline scalars.
    for change in ["acquisition", "input", "deadline"] {
        let other = SqliteStore::open(f.directory.path().join(format!("deadline-{change}.sqlite3"))).unwrap();
        let mut host = admission.host.clone();
        let mut body = request.request().clone();
        match change {
            "acquisition" => host.correlation = id("another-call"),
            "input" => body.request.push_str(" Different exact input."),
            "deadline" => host.limits.deadline_ms = 901,
            _ => unreachable!(),
        }
        let changed = validate_request(body, host, &admission.evidence, &context, f.references.as_ref()).unwrap();
        let changed = SetupStore::new(&other, f.scope()).unwrap()
            .admit(&changed, &admission.evidence, &context, f.references.as_ref()).await.unwrap().snapshot;
        let result = bind_snapshot_deadline_fixture(&held, &operation, &changed);
        if change == "deadline" {
            assert!(matches!(result, Err(PrivateOriginalError::Conflict)));
        } else {
            assert!(matches!(result, Err(PrivateOriginalError::Denied)));
        }
        assert_eq!(held.get(), Some(&900));
    }
    let terminal = bridge.resolve(&snapshot, Resolution::Cancelled(context_reference()), None,
        SetupAccess { evidence: &admission.evidence, context: &context, references: f.references.as_ref() })
        .await.unwrap();
    let unbound = OnceLock::new();
    assert!(matches!(bind_snapshot_deadline_fixture(&unbound, &operation, &terminal),
        Err(PrivateOriginalError::Denied)));
    assert!(unbound.get().is_none());
}
