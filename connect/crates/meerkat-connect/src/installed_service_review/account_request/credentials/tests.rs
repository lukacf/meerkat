//! Closed request and actual checked-selection controls. Package responses are
//! synthetic; these controls do not execute native persistence or a provider.
use super::*;

#[test]
fn credential_request_refuses_values_authority_and_mixed_oauth_without_changing_legacy_parser() {
    let base = json!({"job_id":"checked-job","authentication":{"kind":"declared_secret_slots"},
        "operations":["read"],"agent":"original-agent","expires_in_ms":null});
    assert!(parse(&base.to_string()).is_ok());
    for (field, value) in [("credentials",json!({"KEY":"must-not-enter-the-model"})),
        ("realm",json!("other")),("configuration",json!({})),("slot",json!("KEY")),
        ("connector",json!({})),("grant",json!({})),("operations",json!(["read","read"]))] {
        let mut altered = base.clone(); altered[field] = value;
        assert!(parse(&altered.to_string()).is_err(), "{field}");
    }
    let mut missing = base.clone(); missing.as_object_mut().unwrap().remove("expires_in_ms");
    assert!(parse(&missing.to_string()).is_err());
    assert!(parse(&format!("{{\"job_id\":\"second\",{}", &base.to_string()[1..])).is_err());
    let oauth = json!({"job_id":"checked-job","agent":"original-agent","expires_in_ms":null,
        "connector":{"slot_id":"selected-slot","issuer":"https://issuer.test","client":"client",
            "resource":"https://api.test","scopes":["read"],"strategy_id":"oidc-userinfo-v1",
            "account_selection":null},
        "operations":{"read":{"required_scopes":["read"],"destination_origin":"https://api.test"}}});
    assert!(!is_proposal(&oauth.to_string()).unwrap());
    assert!(super::super::parse(&oauth.to_string()).is_ok());
    assert!(super::super::parse(&base.to_string()).is_err());
}

#[tokio::test]
async fn credential_review_uses_exact_checked_slots_and_launch_and_refuses_retired_or_widened_selection() {
    let root = tempfile::tempdir().unwrap();
    let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(json!({
        "authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
        "operation_tools":{"declared/read":"actual_read"},
        "environment_secret_slots":["FIXTURE_KEY","FIXTURE_TOKEN"]}))).await;
    let selected = fixture.raw_selection().ok().unwrap();
    let selection = CredentialSelection { operations:["actual_read".to_owned()].into(),
        agent:"original-agent".into(),expires_in_ms:None,tool_policy:Some(serde_json::from_value(json!({
            "tools":[{"name":"actual_read","consequence":"r0"}],"catalog":"r0"})).unwrap()) };
    let (config, slots) = checked_configuration(&selected, &selection).unwrap();
    assert_eq!(slots.iter().map(|slot| slot.environment_variable().to_owned()).collect::<BTreeSet<_>>(),
        ["FIXTURE_KEY".to_owned(),"FIXTURE_TOKEN".to_owned()].into());
    let request = credential_review_request(&selected, &config, &selection).unwrap();
    let body = request.request_body.as_ref().unwrap();
    assert_eq!(body["installation"], serde_json::to_value(&config).unwrap());
    assert_eq!(body["phase"], "installed_credential_service_access");
    assert_eq!(body["authentication"], json!({"kind":"declared_secret_slots"}));
    assert_eq!(body["application_selection"]["operations"], json!(["actual_read"]));
    assert!(body["notice"].as_str().unwrap().contains("Saved configuration or credentials can remain"));
    assert!(!body["notice"].as_str().unwrap().contains("Realm configuration is not changed"));
    assert!(selected.public_configuration(&selection.operations).is_err());
    let mut changed = config.clone();
    changed.access = Some(meerkat_core::McpServerAccess::allowing(["other_write".to_owned()]));
    assert!(credential_review_request(&selected, &changed, &selection).is_err());
    let widened = CredentialSelection { operations:["actual_read".to_owned(),"other_write".to_owned()].into(),
        agent:selection.agent.clone(),expires_in_ms:None,tool_policy:None };
    assert!(checked_configuration(&selected, &widened).is_err());
    fixture.owner.retire();
    assert!(credential_review_request(&selected, &config, &selection).is_err());
}

// The package response remains synthetic, but the checked selection, review
// producer, ApprovalService decision, parser and launch derivation are real.
#[cfg(all(unix, feature = "console"))]
#[tokio::test]
async fn credential_review_round_trips_the_installation_reader_without_losing_checked_evidence() {
    use crate::resources::installation_readback::checked_credential_review_for_test as read;
    use meerkat_core::ApprovalService;
    let root = tempfile::tempdir().unwrap();
    let declaration = json!({
        "authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
        "operation_tools":{"declared/read":"actual_read"},
        "environment_secret_slots":["FIXTURE_KEY","FIXTURE_TOKEN"]});
    let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(declaration.clone())).await;
    let selected = fixture.raw_selection().ok().unwrap();
    let selection = CredentialSelection { operations:["actual_read".to_owned()].into(),
        agent:"original-agent".into(),expires_in_ms:None,tool_policy:Some(serde_json::from_value(json!({
            "tools":[{"name":"actual_read","consequence":"r0"}],"catalog":"r0"})).unwrap()) };
    let (config, slots) = checked_configuration(&selected, &selection).unwrap();
    let request = credential_review_request(&selected, &config, &selection).unwrap();
    let approvals = ApprovalService::new();
    let pending = approvals.request(request).unwrap();
    let approved = approvals.decide(&pending.approval_id, ApprovalDecision::Approve,
        ApprovalPrincipalId::new("credential-reader-control").unwrap(), None, None).unwrap();
    let software = &selected.installed;
    let basis = selected.operation_basis();
    let mut result = json!({"scope":"retained_payload_and_launch_binding_only",
        "selection_kind":software.profile,"mcp_path":software.mcp_path,"receipt_path":software.receipt_path,
        "receipt_sha256":software.receipt_sha256,"configuration_sha256":software.configuration_sha256,
        "mcp_configuration":*selected.configuration,"declared_service":declaration,
        "package_sha256":basis["package_sha256"],"effect_sha256":basis["effect_sha256"],
        "plugin_source":basis["source"]["plugin_source"],"acquired_source":basis["source"]["acquired_source"]});
    for key in ["selection_sha256", "manifest_digest", "document_kind", "declaration_digest"] {
        if let Some(value) = basis.get(key) { result[key] = value.clone(); }
    }
    let (observed, recovery) = read(&approved, &software.realm, &result).unwrap();
    assert_eq!(observed, config);
    assert!(recovery.get("public_service").is_none());
    assert_eq!(recovery["credential_service"]["credential_slots"], serde_json::to_value(&slots).unwrap());
    assert_eq!(recovery["software"]["mcp_path"], json!(software.mcp_path));
    assert_eq!(recovery["software"]["receipt_path"], json!(software.receipt_path));
    assert_eq!(recovery["software"]["document_sha256"], software.mcp_document_sha256);
    assert_eq!(recovery["software"]["recovery_request_sha256"], json!(software.recovery_request_sha256));
    for field in ["mcp_path", "receipt_path", "document_sha256", "recovery_request_sha256"] {
        let mut missing = approved.clone();
        missing.request_body.as_mut().unwrap()["software"].as_object_mut().unwrap().remove(field);
        assert!(read(&missing, &software.realm, &result).is_err(), "missing artifact evidence");
    }
    for field in ["receipt_sha256", "effect_sha256", "package_sha256", "mcp_path"] {
        let mut changed = result.clone(); changed[field] = json!("changed");
        assert!(read(&approved, &software.realm, &changed).is_err(), "changed artifact evidence");
    }
    for mode in ["missing", "extra", "wrong-source", "unknown-field", "duplicate", "declared-removed", "declared-auth", "operation", "reviewed-slot", "reviewed-command", "inline-env", "native-secret-env"] {
        let mut record = approved.clone(); let mut value = result.clone();
        let body = record.request_body.as_mut().unwrap();
        match mode {
            "missing" => { body.as_object_mut().unwrap().remove("credential_slots"); },
            "extra" => { body["credential_slots"].as_array_mut().unwrap().push(json!({"source":"selected_environment","declaration":"EXTRA"})); },
            "wrong-source" => body["credential_slots"][0] = json!({"source":"library_field","declaration":{"field_path":["config","key"],"environment_variable":"FIXTURE_KEY"}}),
            "unknown-field" => body["credential_slots"][0]["secret"] = json!("not-an-input"),
            "duplicate" => { let first = body["credential_slots"][0].clone(); body["credential_slots"][1] = first; },
            "declared-removed" => value["declared_service"]["environment_secret_slots"] = json!(["FIXTURE_KEY"]),
            "declared-auth" => value["declared_service"]["authentication"] = json!({"kind":"none"}),
            "operation" => body["application_selection"]["operations"] = json!(["other_write"]),
            "reviewed-slot" => body["installation"]["secret_env"] = json!(["FIXTURE_KEY"]),
            "reviewed-command" => body["installation"]["command"] = json!("/different/command"),
            "inline-env" => value["mcp_configuration"]["mcpServers"][software.server_name.as_str()]["env"] = json!({"PRIVATE":"must-not-load"}),
            "native-secret-env" => value["mcp_configuration"]["mcpServers"][software.server_name.as_str()]["secret_env"] = json!(["FIXTURE_KEY"]),
            _ => unreachable!(),
        }
        if matches!(mode, "inline-env" | "native-secret-env") {
            use sha2::{Digest, Sha256};
            body["software"]["document_sha256"] = json!(format!("{:x}", Sha256::digest(
                serde_json::to_vec(&value["mcp_configuration"]).unwrap())));
        }
        assert!(read(&record, &software.realm, &value).is_err(), "{mode}");
    }
    let mut legacy = approved.clone();
    legacy.request_body.as_mut().unwrap()["phase"] = json!("installed_account_access");
    assert!(read(&legacy, &software.realm, &result).is_err());
}

// The real Core fenced dispatcher owns preparation and currentness checks.
// This inert work policy binds only the exact request call. It supplies no
// captured task, credential persistence owner, service grant or provider.
struct CredentialRequestWork {
    current: bool,
    checks: Arc<std::sync::atomic::AtomicUsize>,
}
struct CredentialRequestPrepared {
    binding: meerkat_core::authorization::PreparedAuthorizationBinding,
    current: bool,
    checks: Arc<std::sync::atomic::AtomicUsize>,
}
impl meerkat_core::authorization::WorkAuthorization for CredentialRequestWork {
    fn prepare(&self, binding: &meerkat_core::authorization::PreparedAuthorizationBinding)
        -> Result<Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
            meerkat_core::authorization::OperationAuthorizationError> {
        use meerkat_core::authorization::{AuthorizationOperation, OperationRefused, OperationRefusalKind};
        if !matches!(&binding.facts().operation, AuthorizationOperation::Tool(tool)
            if tool.name == SETUP_ACCOUNT_REQUEST) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        Ok(Arc::new(CredentialRequestPrepared { binding: binding.clone(),
            current: self.current, checks: self.checks.clone() }))
    }
}
impl meerkat_core::authorization::PreparedOperationAuthorization for CredentialRequestPrepared {
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        meerkat_core::authorization::OperationReviewTier::R1
    }
    fn check_current(&self, binding: &meerkat_core::authorization::PreparedAuthorizationBinding)
        -> Result<(), meerkat_core::authorization::OperationAuthorizationError> {
        self.checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.current || !self.binding.same_operation(binding) {
            return Err(meerkat_core::authorization::OperationRefused::new(
                meerkat_core::authorization::OperationRefusalKind::Denied).into());
        }
        Ok(())
    }
}

async fn credential_operation_dispatch(owner: &Arc<RequestTool>, session: meerkat_core::SessionId,
    job: &str, operations: Value, tool_policy: Option<Value>, work: CredentialRequestWork,
) -> Result<ToolDispatchOutcome, ToolError> {
    use meerkat_core::{ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
        ToolExecutionResolutionContext, authorization::WorkAuthorizationContext,
        exact_operation::OperationExecutionScope, retained_work::RetainedWorkIdentity};
    let retained = RetainedWorkIdentity::new("fixture:credential-request", meerkat_core::RunId::new(),
        Vec::new(), Default::default(), None);
    let work = WorkAuthorizationContext::new(Arc::new(work),
        OperationExecutionScope::Domain).with_retained_work(retained);
    let context = ToolDispatchContext::default().with_runtime_identity(session, None)
        .with_work_authorization(Some(work));
    let mut request = json!({"job_id":job,"authentication":{"kind":"declared_secret_slots"},
        "operations":operations,"agent":"Wrong display name","expires_in_ms":60000});
    if let Some(policy) = tool_policy { request["tool_policy"] = policy; }
    let args = serde_json::value::to_raw_value(&request).unwrap();
    let call = ToolCallView { id:"credential-operation-correction",name:SETUP_ACCOUNT_REQUEST,args:&args };
    let resolution = ToolExecutionResolutionContext::new(ToolDeadlineChain::new(vec![
        ToolDeadlineContributor::finite(ToolDeadlineOwner::CoreToolDispatch, std::time::Duration::from_secs(2))
    ]).unwrap());
    let plan = meerkat_core::resolve_tool_execution_plan_fenced(owner,call,&context,&resolution).unwrap();
    meerkat_core::dispatch_tool_execution_plan_fenced(owner,call,&context,&plan).await
}

fn current_credential_work() -> CredentialRequestWork {
    CredentialRequestWork { current: true, checks: Default::default() }
}

fn credential_request_owner(checked: Arc<crate::service_install::CheckedInstalls>,
    selected: &InstalledConnectorSelection) -> (Arc<RequestTool>, mpsc::Receiver<InstalledCredentialRequest>) {
    let (send, receive) = mpsc::channel(1);
    (Arc::new(RequestTool { checked,realm:selected.installed.realm.clone(),
        session:selected.installed.session.clone(),requests:None,public_requests:None,
        credential_requests:Some(send),tasks:None,application:None }), receive)
}

#[tokio::test]
async fn credential_operation_feedback_uses_raw_names_before_protected_review() {
    let root = tempfile::tempdir().unwrap();
    let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(json!({
        "authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
        "operation_tools":{"read-item":"actual_read","list-items":"actual_list"},
        "environment_secret_slots":["PRIVATE_FIXTURE_KEY"]}))).await;
    let selected = fixture.raw_selection().ok().unwrap();
    let (owner, mut requests) = credential_request_owner(fixture.owner.clone(), &selected);
    for policy in [None, Some(json!({"tools":[{"name":"actual_read","consequence":"r1"}],"catalog":null}))] {
        for operations in [json!(["read-item"]),json!(["unknown_operation"]),json!(["actual_read","unselected_write"])] {
            let error = credential_operation_dispatch(&owner,selected.installed.session.clone(),
                &selected.installed.job_id,operations,policy.clone(),current_credential_work()).await.unwrap_err();
            let ToolError::InvalidArguments { name,reason } = error else {
                panic!("a current credential raw-name mismatch must produce correctable argument feedback")
            };
            assert_eq!(name,SETUP_ACCOUNT_REQUEST);
            assert!(reason.contains("operation_tools values"));
            assert!(reason.contains(r#"["actual_list","actual_read"]"#));
            assert!(reason.contains("has not entered access review"));
            assert!(!reason.contains("PRIVATE_FIXTURE_KEY"));
            assert!(!reason.contains("Wrong display name"));
            assert!(requests.try_recv().is_err());
        }
    }
    // Correct raw names reach the existing missing captured-task refusal.
    // No credential review or write is authorized by an argument correction.
    let error = credential_operation_dispatch(&owner,selected.installed.session.clone(),
        &selected.installed.job_id,json!(["actual_read"]),None,current_credential_work()).await.unwrap_err();
    assert!(matches!(error,ToolError::AccessDenied { .. }));
    assert!(requests.try_recv().is_err());
    let selection = CredentialSelection { operations:["actual_read".to_owned()].into(),
        agent:"Wrong display name".into(),expires_in_ms:Some(60000),tool_policy:None };
    let (config, slots) = checked_configuration(&selected,&selection).unwrap();
    assert_eq!(config.access.as_ref().unwrap().allowed_operations,["actual_read".to_owned()].into());
    assert_eq!(slots.iter().map(ServiceCredentialSlot::environment_variable).collect::<BTreeSet<_>>(),
        ["PRIVATE_FIXTURE_KEY"].into());
    assert!(selected.secret_configuration(&["read-item".to_owned()].into()).is_err(),
        "feedback never translates or admits a declaration ID");
    assert!(selected.secret_configuration(&["actual_read".to_owned(),"unselected_write".to_owned()].into()).is_err());
}

#[tokio::test]
async fn credential_operation_feedback_discloses_no_names_for_unavailable_or_foreign_selection() {
    for mode in ["retired","unknown-job","foreign-session","public-authentication","missing-slots","unsupported-launch"] {
        let root = tempfile::tempdir().unwrap();
        let mut declaration = json!({
            "authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
            "operation_tools":{"read-item":"private_raw_name"},
            "environment_secret_slots":["PRIVATE_FIXTURE_KEY"]});
        if mode=="public-authentication" {
            declaration["authentication"] = json!({"kind":"none"});
        }
        if matches!(mode,"public-authentication"|"missing-slots") {
            declaration.as_object_mut().unwrap().remove("environment_secret_slots");
        }
        let launch = if mode=="unsupported-launch" { json!({"command":"relative-command","args":[]}) }
            else { json!({"command":"/bin/true","args":["not-executed"]}) };
        let fixture = crate::stdio::tests::selected_public_fixture_at(root.path(),Some(declaration),false,
            meerkat_core::SessionId::new(),"selected-owner",launch).await;
        let selected = fixture.raw_selection().ok().unwrap();
        let (owner,mut requests) = credential_request_owner(fixture.owner.clone(),&selected);
        if mode=="retired" { fixture.owner.retire(); }
        let session = if mode=="foreign-session" { meerkat_core::SessionId::new() }
            else { selected.installed.session.clone() };
        let job = if mode=="unknown-job" { "unknown-job" } else { &selected.installed.job_id };
        let error = credential_operation_dispatch(&owner,session,job,json!(["read-item"]),None,
            current_credential_work()).await.unwrap_err();
        if mode=="foreign-session" { assert!(matches!(&error,ToolError::AccessDenied { .. })); }
        else { assert!(matches!(&error,ToolError::Unavailable { .. }),"{mode}"); }
        assert!(!error.to_string().contains("private_raw_name"),"{mode}");
        assert!(!error.to_string().contains("PRIVATE_FIXTURE_KEY"),"{mode}");
        assert!(requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn credential_operation_feedback_preserves_argument_and_native_currentness_refusals() {
    let root = tempfile::tempdir().unwrap();
    let fixture = crate::stdio::tests::selected_public_fixture(root.path(),Some(json!({
        "authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
        "operation_tools":{"read-item":"private_raw_name"},
        "environment_secret_slots":["PRIVATE_FIXTURE_KEY"]}))).await;
    let selected = fixture.raw_selection().ok().unwrap();
    let (owner,mut requests) = credential_request_owner(fixture.owner.clone(),&selected);
    for operations in [json!(["read-item","read-item"]),json!([]),json!(["invalid/name"])] {
        let error = credential_operation_dispatch(&owner,selected.installed.session.clone(),
            &selected.installed.job_id,operations,None,current_credential_work()).await.unwrap_err();
        assert!(matches!(&error,ToolError::InvalidArguments { .. }));
        assert!(!error.to_string().contains("private_raw_name"));
        assert!(!error.to_string().contains("PRIVATE_FIXTURE_KEY"));
        assert!(requests.try_recv().is_err());
    }
    let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let error = credential_operation_dispatch(&owner,selected.installed.session.clone(),
        &selected.installed.job_id,json!(["read-item"]),None,
        CredentialRequestWork { current: false, checks: checks.clone() }).await.unwrap_err();
    assert!(checks.load(std::sync::atomic::Ordering::SeqCst)>0,
        "the fenced dispatcher must consult the actual prepared work owner");
    assert!(!matches!(&error,ToolError::InvalidArguments { .. }),
        "native refusal must precede request correction");
    assert!(!error.to_string().contains("private_raw_name"));
    assert!(!error.to_string().contains("PRIVATE_FIXTURE_KEY"));
    assert!(requests.try_recv().is_err());
}


#[tokio::test]
async fn console_service_policy_refuses_incomplete_credential_review_without_touching_values() {
    let root = tempfile::tempdir().unwrap();
    let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(json!({
        "authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
        "operation_tools":{"declared/read":"actual_read"},"environment_secret_slots":["FIXTURE_KEY"]}))).await;
    let selected = fixture.raw_selection().ok().unwrap();
    let mut selection = CredentialSelection { operations:["actual_read".to_owned()].into(),
        agent:"original-agent".into(),expires_in_ms:None,tool_policy:None };
    let (config, _) = checked_configuration(&selected, &selection).unwrap();
    for policy in [None, Some(json!({"tools":[{"name":"actual_read","consequence":"r0"}],"catalog":null})),
        Some(json!({"tools":[{"name":"declared/read","consequence":"r0"}],"catalog":"r0"}))] {
        selection.tool_policy = policy.map(|value| serde_json::from_value(value).unwrap());
        assert!(credential_review_request(&selected, &config, &selection).is_err(),
            "an incomplete Console permission proposal must not create a credential review");
    }
    selection.tool_policy = Some(serde_json::from_value(json!({
        "tools":[{"name":"actual_read","consequence":"r0"}],"catalog":"r0"})).unwrap());
    let request = credential_review_request(&selected, &config, &selection).unwrap();
    let body = request.request_body.unwrap();
    assert_eq!(body["application_selection"]["tool_policy"], serde_json::to_value(&selection.tool_policy).unwrap());
    assert!(body.get("credential_values").is_none());
    fixture.owner.retire();
    assert!(credential_review_request(&selected, &config, &selection).is_err());
}
