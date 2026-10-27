//! A fixture for the host's independently authenticated invocation custody.
//! This exercises the private protocol, not production availability or a real
//! runtime's authentication. No authority is derived from model tool arguments.
use super::*;
use meerkat_authorization_contracts::work_association::OriginalWorkRef;
use std::sync::Mutex;

mod no_operations;

struct CustodyHost {
    original: Host,
    // Captured once from authenticated invocation admission. Candidate loading
    // cannot replace this value, even if the replacement is well-formed historical native data.
    expected: OriginalWorkRef,
    candidate: Mutex<SetupRequestEvidence>,
    // Allocated by the trusted host for one invocation, independent of user
    // content, audience and model tool-call IDs. Stable over this fixture's retries.
    correlation: Id,
}
impl CustodyHost {
    fn new(f: &Fixture, host_correlation: &str) -> Self {
        Self {
            original: Host {
                session: SessionId::new(),
                interaction: InteractionId::new(),
                evidence: f.host.evidence.clone(),
                authorization: f.host.authorization.clone(),
                target: f.host.target.clone(),
                audience: f.host.audience.clone(),
                calls: 0.into(),
                mode: 0.into(),
            },
            expected: f.host.evidence.original_work().clone(),
            candidate: Mutex::new(f.host.evidence.clone()),
            correlation: id(host_correlation),
        }
    }
    fn tool(self: &Arc<Self>, f: &Fixture, store: Arc<dyn ToolkitStore>) -> FindAndInstallTool {
        FindAndInstallTool::new(
            store,
            f.scope(),
            self.clone(),
            f.references.clone(),
            Duration::from_secs(2),
        )
        .unwrap()
    }
}
#[async_trait]
impl SetupToolHost for CustodyHost {
    async fn admit_invocation(
        &self,
        call_id: &str,
        context: &ToolDispatchContext,
    ) -> Result<HostToolAdmission, ToolAdmissionError> {
        self.original.calls.fetch_add(1, Ordering::SeqCst);
        if call_id != "same-model-call"
            || context.origin_session_id() != Some(&self.original.session)
            || context.interaction_lineage_id() != Some(self.original.interaction)
        {
            return Err(ToolAdmissionError::Denied);
        }
        let evidence = self.candidate.lock().unwrap().clone();
        if evidence.original_work() != &self.expected
            || evidence.association() != self.original.evidence.association()
        {
            return Err(ToolAdmissionError::Denied);
        }
        let authorization = context
            .work_authorization()
            .ok_or(ToolAdmissionError::Denied)?;
        let mut admission = self.original.admission(call_id);
        evidence
            .check_context(&admission.context.borrowed(authorization))
            .map_err(|_| ToolAdmissionError::Denied)?;
        admission.evidence = evidence;
        admission.host.correlation = self.correlation.clone();
        admission.host.continuation.delivery_key = id(&format!("delivery-{}", self.correlation));
        Ok(admission)
    }
}

async fn invoke_custody(
    tool: &FindAndInstallTool,
    context: &ToolDispatchContext,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw = RawValue::from_string(input().to_string()).unwrap();
    tool.dispatch_protocol_fixture(
        ToolCallView {
            id: "same-model-call",
            name: FIND_AND_INSTALL,
            args: &raw,
        },
        context,
    )
    .await
}

async fn saved_protocol(f: &Fixture) -> Value {
    let records = f.records().await;
    let mut events = Vec::new();
    let mut effects = Vec::new();
    for record in &records {
        events.push(f.store.read_events(&record.key, 0, 100).await.unwrap());
        effects.push(f.store.list_effects(&record.key, None, 100).await.unwrap());
    }
    serde_json::json!({"records": records, "events": events, "effects": effects})
}

#[tokio::test]
async fn invocation_custody_rejects_other_native_evidence_before_references_or_storage() {
    let f = Fixture::new();
    let host = Arc::new(CustodyHost::new(&f, "host-invocation-a"));
    let expected = host.expected.clone();
    let first = reply(
        &invoke_custody(&host.tool(&f, f.store.clone()), &host.original.context())
            .await
            .unwrap(),
    );
    assert_eq!(first["created"], true);
    let persisted = saved_protocol(&f).await;
    let reopened = Arc::new(SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap());
    let tool = host.tool(&f, reopened.clone());
    let replay = reply(
        &invoke_custody(&tool, &host.original.context())
            .await
            .unwrap(),
    );
    assert_eq!(replay["created"], false);
    assert_eq!(replay["operation"], first["operation"]);
    assert_eq!(saved_protocol(&f).await, persisted);

    // Constructor capability inspection is allowed. Any operational store
    // method, including read-only lookups and the clock, panics below.
    let guarded = host.tool(&f, Arc::new(no_operations::NoOperations(reopened.clone())));
    for change in 0..3 {
        let mut claims = f.host.evidence.association().candidate().clone();
        match change {
            0 => {
                claims.original_work.work =
                    meerkat_authorization_contracts::evidence::EvidenceId::new("another-original")
                        .unwrap()
            }
            1 => claims.requester.kind = PrincipalKind::PersonalAgent,
            2 => {
                claims.original_work.authority =
                    native_identity::principal(PrincipalKind::ServiceAccount, "another-host")
            }
            _ => unreachable!(),
        }
        let association =
            meerkat_authorization_contracts::work_association::InputAuthorityAssociation::new(
                claims,
            )
            .unwrap();
        let replacement =
            SetupRequestEvidence::new(association, f.host.evidence.constraints().clone()).unwrap();
        // Encoding validation is data validation, never a native permission.
        assert!(replacement.association() != f.host.evidence.association());
        *host.candidate.lock().unwrap() = replacement;
        let checks = f.references.checks.load(Ordering::SeqCst);
        let calls = host.original.calls.load(Ordering::SeqCst);
        assert!(matches!(
            invoke_custody(&guarded, &host.original.context()).await,
            Err(ToolError::AccessDenied { .. })
        ));
        assert_eq!(host.original.calls.load(Ordering::SeqCst), calls + 1);
        assert_eq!(f.references.checks.load(Ordering::SeqCst), checks);
        assert_eq!(host.expected, expected);
        assert_eq!(saved_protocol(&f).await, persisted);
    }
    *host.candidate.lock().unwrap() = f.host.evidence.clone();
    let restored = reply(
        &invoke_custody(&tool, &host.original.context())
            .await
            .unwrap(),
    );
    assert_eq!(restored["operation"], first["operation"]);
    assert_eq!(restored["created"], false);
    assert_eq!(saved_protocol(&f).await, persisted);
    assert_eq!(f.records().await.len(), 1);
}

#[tokio::test]
async fn same_principal_and_model_call_id_keep_distinct_host_invocations() {
    let f = Fixture::new();
    let first = Arc::new(CustodyHost::new(&f, "host-invocation-a"));
    let second = Arc::new(CustodyHost::new(&f, "host-invocation-b"));
    assert_eq!(first.expected, second.expected);
    assert_eq!(
        first.original.evidence.association().candidate().requester,
        second.original.evidence.association().candidate().requester
    );
    assert_ne!(first.original.session, second.original.session);
    assert_ne!(first.original.interaction, second.original.interaction);
    assert_ne!(first.correlation, second.correlation);
    let a = reply(
        &invoke_custody(&first.tool(&f, f.store.clone()), &first.original.context())
            .await
            .unwrap(),
    );
    let b = reply(
        &invoke_custody(
            &second.tool(&f, f.store.clone()),
            &second.original.context(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(a["created"], true);
    assert_eq!(b["created"], true);
    assert_ne!(a["operation"], b["operation"]);
    let persisted = saved_protocol(&f).await;
    let checks = f.references.checks.load(Ordering::SeqCst);
    assert!(matches!(
        invoke_custody(&first.tool(&f, f.store.clone()), &second.original.context()).await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(f.references.checks.load(Ordering::SeqCst), checks);
    assert_eq!(saved_protocol(&f).await, persisted);
    let reopened = Arc::new(SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap());
    for (host, original) in [(&first, a), (&second, b)] {
        let replay = reply(
            &invoke_custody(&host.tool(&f, reopened.clone()), &host.original.context())
                .await
                .unwrap(),
        );
        assert_eq!(replay["created"], false);
        assert_eq!(replay["operation"], original["operation"]);
    }
    assert_eq!(saved_protocol(&f).await, persisted);
    let records = f.records().await;
    assert_eq!(records.len(), 2);
    for record in records {
        let effects = f.store.list_effects(&record.key, None, 100).await.unwrap();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].intent.kind, "work_admission_requested");
    }
}
