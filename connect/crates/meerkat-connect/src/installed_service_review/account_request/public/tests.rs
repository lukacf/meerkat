//! Consumer and protocol controls. Synthetic package responses are not live
//! package, Console-login, native grant or original-task completion evidence.
use super::*;

fn console_policy(name: &str) -> Option<ServiceToolPolicySelection> {
    Some(serde_json::from_value(json!({"tools":[{"name":name,"consequence":"r0"}],"catalog":"r0"})).unwrap())
}

#[test]
fn public_request_requires_exact_operations_and_explicit_lifetime_without_account_fields() {
    let base = json!({"job_id":"checked-job","operations":["read_public_feed"],"agent":"original-agent","expires_in_ms":null});
    assert!(parse(&base.to_string()).is_ok());
    for (field, value) in [("connector", json!({"slot_id":"invented"})),
        ("command",json!("/bin/sh")),("credentials",json!({})),("authority",json!({})),
        ("operations",json!(["read_public_feed","read_public_feed"])),
        ("operations",json!([])),("expires_in_ms",json!(0)),("agent",json!(""))] {
        let mut invalid = base.clone(); invalid[field] = value;
        assert!(parse(&invalid.to_string()).is_err(), "{field}");
    }
    let mut missing = base.clone(); missing.as_object_mut().unwrap().remove("expires_in_ms");
    assert!(parse(&missing.to_string()).is_err());
    assert!(parse(r#"{"job_id":"one","job_id":"two","operations":["read"],"agent":"a","expires_in_ms":null}"#).is_err());
}

#[tokio::test]
async fn public_review_keeps_selected_provenance_and_refuses_changed_or_bound_sources() {
    let root = tempfile::tempdir().unwrap();
    let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(json!({
        "authentication":{"kind":"none"},"operation_tools":{"declared/read":"actual_read"}}))).await;
    let selected = fixture.raw_selection().ok().unwrap();
    let selection = PublicSelection { tool_policy: console_policy("actual_read"), operations:["actual_read".to_owned()].into(),
        agent:"waiting-agent".into(),expires_in_ms:None };
    let config = selected.public_configuration(&selection.operations).unwrap();
    let retained = selected.clone();
    assert_eq!(retained.public_configuration(&selection.operations).unwrap(),config);
    let request = public_review_request(&selected.clone().into(),&config,&selection).unwrap();
    let body = request.request_body.as_ref().unwrap();
    assert_eq!(body["phase"],"installed_public_service_access");
    assert_eq!(body["authentication"],json!({"kind":"none"}));
    assert_eq!(body["operation_basis"]["kind"],"selected_manifest_declarations");
    assert_eq!(body["operation_basis"],selected.operation_basis());
    assert_eq!(body["application_selection"],json!({"agent":"waiting-agent","expires_in_ms":null,"operations":["actual_read"],"tool_policy":selection.tool_policy}));
    assert!(body.get("account").is_none()); assert!(body.get("connector").is_none());
    assert!(config.connector.is_none());
    let mut changed = config.clone(); changed.access = Some(meerkat_core::McpServerAccess::allowing(["unreviewed_write".to_owned()]));
    assert!(public_review_request(&selected.clone().into(),&changed,&selection).is_err());
    let binding_root = tempfile::tempdir().unwrap();
    let binding = crate::stdio::tests::selected_public_fixture(binding_root.path(), Some(json!({
        "authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
        "operation_tools":{"declared/read":"actual_read"}}))).await;
    let binding = binding.raw_selection().ok().unwrap();
    assert!(binding.native_connector.is_none());
    assert!(public_review_request(&binding.clone().into(),&config,&selection).is_err(), "absent native connector cannot imply no provider authentication");
    fixture.owner.retire();
    assert!(public_review_request(&selected.clone().into(),&config,&selection).is_err());
    assert!(retained.public_configuration(&selection.operations).is_err(),
        "an owned publication predicate still consults the same retired source");
    assert_eq!(body["operation_basis"]["kind"],"selected_manifest_declarations", "old review remains historical evidence only");
}

#[tokio::test]
async fn public_reply_driver_joins_cancellation_and_preserves_the_presenter_for_denial() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let root = tempfile::tempdir().unwrap();
        let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(json!({
            "authentication":{"kind":"none"},"operation_tools":{"declared/read":"actual_read"}}))).await;
        let selected = fixture.raw_selection().ok().unwrap();
        let selection = PublicSelection { tool_policy: console_policy("actual_read"), operations:["actual_read".to_owned()].into(),
            agent:"waiting-agent".into(),expires_in_ms:None };
        let config = selected.public_configuration(&selection.operations).unwrap();
        let approval_root = root.path().join("approvals");
        std::fs::create_dir(&approval_root).unwrap();
        let (approvals, presenter) = LocalApprovals::create(&approval_root).unwrap();
        approvals.bind_session(&selected.installed.session).unwrap();
        let mut presenter = Some(presenter);
        // Exercise the real ApprovalService/prompt driver with the public reply
        // type. This is transport settlement, not a verified Console Human or
        // native task-grant fixture. The native task control lives in MobKit.
        for cancel in [true, false] {
            let (bridge, mut prompts, _retire) = PromptBridge::channel();
            let (mut response, receive) = oneshot::channel::<PublicReply>();
            let (control, cancellation) = ReviewCancellation::channel();
            let settled = AtomicBool::new(false);
            let request = public_review_request(&selected.clone().into(), &config, &selection).unwrap();
            let review = async {
                let result = approvals.review_access_cancellable(request,
                    cancellation.cancelled(), |id| cancellation.approval_created(id)).await;
                settled.store(true, Ordering::SeqCst);
                let record = result?;
                assert_eq!(record.status, ApprovalStatus::Denied);
                Ok(PublicReply { state:PublicState::Denied,access_reviewed:true,
                    grant_issued:false,fresh_instruction_retained:false,ready:false })
            };
            let refresh = || -> futures_util::future::BoxFuture<'static, Result<(), ConnectError>> {
                Box::pin(async { Ok(()) })
            };
            let closed = std::future::pending();
            tokio::pin!(closed);
            let driver = console::drive_review_cancellable(review,&approvals,&mut presenter,
                &bridge,&refresh,&mut response,closed.as_mut(),None,Some(control),None);
            let answer = async {
                let mut exchange = Some(prompts.recv().await.unwrap());
                let prompt = &exchange.as_ref().unwrap().prompt;
                assert_eq!(prompt["body"]["phase"],"installed_public_service_access");
                if cancel {
                    drop(receive);
                    None
                } else {
                    let id = prompt["approval_id"].as_str().unwrap().to_owned();
                    console::answer_retained(12,&mut exchange,&serde_json::to_vec(&json!({
                        "review_id":12,"approval_id":id,"decision":"deny"})).unwrap()).unwrap();
                    // join! retains the completed branch's receiver until the
                    // real durable denied decision and driver both settle.
                    Some(receive)
                }
            };
            let (result, _retained_receiver) = tokio::join!(driver, answer);
            assert!(settled.load(Ordering::SeqCst),"entered approval wait must be joined on caller loss");
            if cancel {
                assert!(result.unwrap().is_none());
            } else {
                let reply = serde_json::to_value(result.unwrap().unwrap()).unwrap();
                assert_eq!(reply,json!({"state":"denied","access_reviewed":true,
                    "grant_issued":false,"fresh_instruction_retained":false,"ready":false}));
            }
            assert!(presenter.is_some(),"one public request cannot consume the host presenter");
            approvals.check_session(&selected.installed.session).unwrap();
            assert!(!approvals.has_uncertain_dispatch());
        }
    }).await.expect("public approval transport failed to settle");
}


#[tokio::test]
async fn public_reply_driver_polls_expiry_while_shown_and_preserves_the_next_review() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let root = tempfile::tempdir().unwrap();
        let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(json!({
            "authentication":{"kind":"none"},"operation_tools":{"declared/read":"actual_read"}}))).await;
        let selected = fixture.raw_selection().ok().unwrap();
        let selection = PublicSelection { tool_policy: console_policy("actual_read"), operations:["actual_read".to_owned()].into(),
            agent:"waiting-agent".into(),expires_in_ms:None };
        let config = selected.public_configuration(&selection.operations).unwrap();
        let approval_root = root.path().join("approvals");
        std::fs::create_dir(&approval_root).unwrap();
        let (approvals, presenter) = LocalApprovals::create(&approval_root).unwrap();
        approvals.bind_session(&selected.installed.session).unwrap();
        let mut presenter = Some(presenter);
        for expires in [true, false] {
            let (bridge, mut prompts, _retire) = PromptBridge::channel();
            let (mut response, receive) = oneshot::channel::<PublicReply>();
            let (control, cancellation) = ReviewCancellation::channel();
            let settled = AtomicBool::new(false);
            let mut request = public_review_request(&selected.clone().into(), &config, &selection).unwrap();
            if expires {
                request.expires_at = Some((SystemTime::now() + Duration::from_millis(500)).into());
            }
            let review = async {
                let result = approvals.review_access_cancellable(request,
                    cancellation.cancelled(), |id| cancellation.approval_created(id)).await;
                settled.store(true, Ordering::SeqCst);
                result
            };
            let refresh = || -> futures_util::future::BoxFuture<'static, Result<(), ConnectError>> {
                Box::pin(async { Ok(()) })
            };
            let closed = std::future::pending();
            tokio::pin!(closed);
            let driver = console::drive_review_cancellable(review,&approvals,&mut presenter,
                &bridge,&refresh,&mut response,closed.as_mut(),None,Some(control),None);
            let presentation = async {
                let mut exchange = Some(prompts.recv().await.unwrap());
                let prompt = &exchange.as_ref().unwrap().prompt;
                assert_eq!(prompt["body"]["phase"],"installed_public_service_access");
                if !expires {
                    let id = prompt["approval_id"].as_str().unwrap().to_owned();
                    console::answer_retained(124,&mut exchange,&serde_json::to_vec(&json!({
                        "review_id":124,"approval_id":id,"decision":"deny"})).unwrap()).unwrap();
                }
                // Keep both channels alive without answering the first prompt.
                // Only the actual approval future's deadline can settle it.
                (exchange, receive)
            };
            let (result, (mut exchange, _retained_receiver)) = tokio::join!(driver, presentation);
            assert!(settled.load(Ordering::SeqCst),"the actual approval future must finish");
            if expires {
                assert!(matches!(result, Err(ConnectError::InvalidInput(ref message))
                    if message == "The access review expired without a returned decision"));
                let shown = exchange.as_ref().expect("the expired prompt was actually displayed");
                assert!(shown.response.is_closed(),"expiry must retire the displayed answer channel");
                let id = shown.prompt["approval_id"].as_str().unwrap().to_owned();
                assert!(console::answer_retained(124,&mut exchange,&serde_json::to_vec(&json!({
                    "review_id":124,"approval_id":id,"decision":"approve"})).unwrap()).is_err(),
                    "a late answer cannot enter the expired review");
            } else {
                assert_eq!(result.unwrap().unwrap().status,ApprovalStatus::Denied,
                    "the same presenter must still serve the next real review");
                assert!(exchange.is_none());
            }
            assert!(prompts.try_recv().is_err(),"one review must not display a duplicate prompt");
            assert!(presenter.is_some(),"expiry belongs to the request, not the host presenter");
            approvals.check_session(&selected.installed.session).unwrap();
            assert!(!approvals.has_uncertain_dispatch());
        }
    }).await.expect("displayed public review did not settle at its actual approval deadline");
}


#[tokio::test]
async fn remote_public_review_retains_export_research_and_exact_consumer_without_fake_software() {
    for declaration in [false, true] {
        let root=tempfile::tempdir().unwrap();
        let fixture=crate::stdio::tests::remote_public_fixture_kind(root.path(), declaration).await;
        let selected=fixture.public_remote();
        let selection=PublicSelection { tool_policy: console_policy("actual.lookup"), operations:["actual.lookup".to_owned()].into(),
            agent:"original-agent".into(),expires_in_ms:Some(60000) };
        let config=selected.public_configuration(&selection.operations).unwrap();
        let review=public_review_request(&selected,&config,&selection).unwrap();
        let body=review.request_body.as_ref().unwrap();
        assert_eq!(body["operation_basis"]["kind"],"remote_recipe_declarations");
        let metadata = &body["operation_basis"]["declared_remote_service"];
        assert_eq!(metadata.get("selection_kind").and_then(Value::as_str), if declaration { Some("remote_declaration") } else { None });
        assert_eq!(body["remote_export"],selected.review_source()["remote_export"]);
        assert!(body.get("software").is_none()); assert!(body.get("account").is_none());
        assert_eq!(body["application_selection"],json!({"agent":"original-agent","operations":["actual.lookup"],"expires_in_ms":60000,"tool_policy":selection.tool_policy}));
        assert_eq!(body["installation"],serde_json::to_value(&config).unwrap());
        let widened=PublicSelection { tool_policy: None, operations:["actual.lookup".to_owned(),"other.write".to_owned()].into(),
            agent:selection.agent.clone(),expires_in_ms:selection.expires_in_ms };
        assert!(public_review_request(&selected,&config,&widened).is_err());
        fixture.owner.retire();
        assert!(public_review_request(&selected,&config,&selection).is_err());
        assert!(body.get("remote_export").is_some(),"retained review is historical evidence after disposal");
    }
}

#[tokio::test]
async fn public_review_retains_software_declaration_without_full_activation_or_schema_observation() {
    let root=tempfile::tempdir().unwrap();
    let fixture=crate::stdio::tests::selected_software_fixture(root.path()).await;
    let selected=fixture.raw_selection().ok().unwrap();
    let selection=PublicSelection { tool_policy: console_policy("actual_read"),operations:["actual_read".to_owned()].into(),
        agent:"waiting-agent".into(),expires_in_ms:Some(60000)};
    let config=selected.public_configuration(&selection.operations).unwrap();
    let request=public_review_request(&selected.clone().into(),&config,&selection).unwrap();
    let body=request.request_body.as_ref().unwrap();
    assert_eq!(body["phase"],"installed_public_service_access");
    assert_eq!(body["operation_basis"],selected.operation_basis());
    assert_eq!(body["operation_basis"]["kind"],"selected_software_declarations");
    assert!(body["operation_basis"].get("manifest_digest").is_none());
    assert_eq!(body["application_selection"],json!({"operations":["actual_read"],"agent":"waiting-agent","expires_in_ms":60000,"tool_policy":selection.tool_policy}));
    assert!(config.connector.is_none());
    assert!(body.get("account").is_none());
    assert!(body.get("observed_schema").is_none());
    let other=PublicSelection { tool_policy: None,operations:["unselected_write".to_owned()].into(),
        agent:selection.agent.clone(),expires_in_ms:selection.expires_in_ms};
    assert!(public_review_request(&selected.clone().into(),&config,&other).is_err());
    fixture.owner.retire();
    assert!(public_review_request(&selected.clone().into(),&config,&selection).is_err());
}

#[test]
fn public_tool_policy_is_optional_proposal_not_candidate_or_catalog_session_authority() {
    let mut requested = json!({"job_id":"checked","operations":["read"],"agent":"original","expires_in_ms":null});
    assert!(parse(&requested.to_string()).unwrap().tool_policy.is_none());
    requested["tool_policy"] = json!({"tools":[{"name":"named.alias","consequence":"r0"}],"catalog":"r1"});
    assert!(parse(&requested.to_string()).unwrap().tool_policy.is_some());
    requested["tool_policy"]["catalog_session"] = json!("helper-session");
    assert!(parse(&requested.to_string()).is_err());
}


// Inert work ownership for the real Core fenced dispatcher. This binds each
// decision to its exact call, but deliberately supplies no captured task or
// service grant. It is not an installed application or native admission proof.
struct PublicRequestWork;
struct PublicRequestPrepared(meerkat_core::authorization::PreparedAuthorizationBinding);
impl meerkat_core::authorization::WorkAuthorization for PublicRequestWork {
    fn prepare(&self, binding: &meerkat_core::authorization::PreparedAuthorizationBinding)
        -> Result<Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
            meerkat_core::authorization::OperationAuthorizationError> {
        use meerkat_core::authorization::{AuthorizationOperation, OperationRefused, OperationRefusalKind};
        if !matches!(&binding.facts().operation, AuthorizationOperation::Tool(tool)
            if tool.name == SETUP_PUBLIC_SERVICE_REQUEST) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        Ok(Arc::new(PublicRequestPrepared(binding.clone())))
    }
}
impl meerkat_core::authorization::PreparedOperationAuthorization for PublicRequestPrepared {
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        meerkat_core::authorization::OperationReviewTier::R1
    }
    fn check_current(&self, binding: &meerkat_core::authorization::PreparedAuthorizationBinding)
        -> Result<(), meerkat_core::authorization::OperationAuthorizationError> {
        if !self.0.same_operation(binding) {
            return Err(meerkat_core::authorization::OperationRefused::new(
                meerkat_core::authorization::OperationRefusalKind::Denied).into());
        }
        Ok(())
    }
}

async fn public_operation_dispatch(owner: &Arc<RequestTool>, session: meerkat_core::SessionId,
    job: &str, operations: Value, tool_policy: Option<Value>) -> Result<ToolDispatchOutcome, ToolError> {
    use meerkat_core::{ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
        ToolExecutionResolutionContext, authorization::WorkAuthorizationContext,
        exact_operation::OperationExecutionScope, retained_work::RetainedWorkIdentity};
    let retained = RetainedWorkIdentity::new("fixture:public-request", meerkat_core::RunId::new(),
        Vec::new(), Default::default(), None);
    let work = WorkAuthorizationContext::new(Arc::new(PublicRequestWork),
        OperationExecutionScope::Domain).with_retained_work(retained);
    let context = ToolDispatchContext::default().with_runtime_identity(session, None)
        .with_work_authorization(Some(work));
    let mut request = json!({"job_id":job,"operations":operations,
        "agent":"Wrong display name","expires_in_ms":60000});
    if let Some(policy) = tool_policy { request["tool_policy"] = policy; }
    let args = serde_json::value::to_raw_value(&request).unwrap();
    let call = ToolCallView { id:"public-operation-correction",name:SETUP_PUBLIC_SERVICE_REQUEST,args:&args };
    let resolution = ToolExecutionResolutionContext::new(ToolDeadlineChain::new(vec![
        ToolDeadlineContributor::finite(ToolDeadlineOwner::CoreToolDispatch, Duration::from_secs(2))
    ]).unwrap());
    let plan = meerkat_core::resolve_tool_execution_plan_fenced(owner,call,&context,&resolution).unwrap();
    meerkat_core::dispatch_tool_execution_plan_fenced(owner,call,&context,&plan).await
}

fn public_request_owner(checked: Arc<crate::service_install::CheckedInstalls>,
    selected: &crate::service_install::InstalledConnectorSelection)
    -> (Arc<RequestTool>, mpsc::Receiver<InstalledPublicRequest>) {
    let (send, receive) = mpsc::channel(1);
    (Arc::new(RequestTool { checked,realm:selected.installed.realm.clone(),
        session:selected.installed.session.clone(),requests:None,public_requests:Some(send),
        credential_requests:None,tasks:None,application:None }), receive)
}

#[tokio::test]
async fn public_operation_feedback_uses_raw_names_before_target_or_access_review() {
    let root = tempfile::tempdir().unwrap();
    let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(json!({
        "authentication":{"kind":"none"},"operation_tools":{
            "read-item":"actual_read","list-items":"actual_list"}}))).await;
    let selected = fixture.raw_selection().ok().unwrap();
    let (owner, mut requests) = public_request_owner(fixture.owner.clone(), &selected);
    for policy in [None, Some(json!({"tools":[{"name":"actual_read","consequence":"r1"}],"catalog":null}))] {
        for operations in [json!(["read-item"]),json!(["unknown_operation"]),json!(["actual_read","unselected_write"])] {
            let error = public_operation_dispatch(&owner,selected.installed.session.clone(),
                &selected.installed.job_id,operations,policy.clone()).await.unwrap_err();
            let ToolError::InvalidArguments { name,reason } = error else {
                panic!("a current raw-name mismatch must produce correctable argument feedback")
            };
            assert_eq!(name,SETUP_PUBLIC_SERVICE_REQUEST);
            assert!(reason.contains("operation_tools values"));
            assert!(reason.contains(r#"["actual_list","actual_read"]"#));
            assert!(reason.contains("has not entered access review"));
            assert!(!reason.contains("Wrong display name"));
            assert!(requests.try_recv().is_err());
        }
    }
    // A corrected subset passes operation validation and reaches the existing
    // missing captured-task refusal. This fixture cannot issue access or fake
    // consumer custody; the exact consumer mismatch has its separate control.
    let error = public_operation_dispatch(&owner,selected.installed.session.clone(),
        &selected.installed.job_id,json!(["actual_read"]),None).await.unwrap_err();
    assert!(matches!(error,ToolError::AccessDenied { .. }));
    assert!(requests.try_recv().is_err());
    let config = selected.public_configuration(&["actual_read".to_owned()].into()).unwrap();
    assert_eq!(config.access.unwrap().allowed_operations,
        ["actual_read".to_owned()].into());
    assert!(selected.public_configuration(&["read-item".to_owned()].into()).is_err(),
        "feedback never translates or admits a declaration ID");
    let target = validate_captured_consumer(SETUP_PUBLIC_SERVICE_REQUEST,
        "Wrong display name","fixture-native-member").unwrap_err();
    assert!(matches!(target,ToolError::InvalidArguments { reason,.. }
        if reason.contains("fixture-native-member")));
}

#[tokio::test]
async fn public_operation_feedback_discloses_no_names_for_unavailable_or_foreign_selection() {
    for mode in ["retired","unknown-job","foreign-session","authentication"] {
        let root = tempfile::tempdir().unwrap();
        let authentication = if mode=="authentication" {
            json!({"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"})
        } else { json!({"kind":"none"}) };
        let fixture = crate::stdio::tests::selected_public_fixture(root.path(),Some(json!({
            "authentication":authentication,"operation_tools":{"read-item":"private_raw_name"}}))).await;
        let selected = fixture.raw_selection().ok().unwrap();
        let (owner,mut requests) = public_request_owner(fixture.owner.clone(),&selected);
        if mode=="retired" { fixture.owner.retire(); }
        let session = if mode=="foreign-session" { meerkat_core::SessionId::new() }
            else { selected.installed.session.clone() };
        let job = if mode=="unknown-job" { "unknown-job" } else { &selected.installed.job_id };
        let error = public_operation_dispatch(&owner,session,job,json!(["read-item"]),None).await.unwrap_err();
        if mode=="foreign-session" { assert!(matches!(&error,ToolError::AccessDenied { .. })); }
        else { assert!(matches!(&error,ToolError::Unavailable { .. }),"{mode}"); }
        assert!(!error.to_string().contains("private_raw_name"),"{mode}");
        assert!(requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn public_operation_feedback_keeps_remote_declared_ceilings() {
    for declaration in [false,true] {
        let root = tempfile::tempdir().unwrap();
        let fixture = crate::stdio::tests::remote_public_fixture_kind(root.path(),declaration).await;
        let selected = fixture.public_remote();
        assert_eq!(selected.public_operations().unwrap(),["actual.lookup".to_owned()].into());
        assert!(selected.public_configuration(&["read-item".to_owned()].into()).is_err());
        assert!(selected.public_configuration(&["actual.lookup".to_owned(),"other.write".to_owned()].into()).is_err());
        fixture.owner.retire();
        assert!(selected.public_operations().is_err());
    }
}


#[tokio::test]
async fn console_service_policy_refuses_omitted_or_partial_proposals_before_public_review() {
    let root = tempfile::tempdir().unwrap();
    let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(json!({
        "authentication":{"kind":"none"},"operation_tools":{"declared/read":"actual_read"}}))).await;
    let selected: PublicServiceSelection = fixture.raw_selection().ok().unwrap().into();
    let mut selection = PublicSelection { tool_policy: None,
        operations:["actual_read".to_owned()].into(),agent:"waiting-agent".into(),expires_in_ms:None };
    let config = selected.public_configuration(&selection.operations).unwrap();
    for policy in [None, Some(json!({"tools":[{"name":"actual_read","consequence":"r0"}],"catalog":null})),
        Some(json!({"tools":[],"catalog":"r0"})),
        Some(json!({"tools":[{"name":"declared/read","consequence":"r0"}],"catalog":"r0"}))] {
        selection.tool_policy = policy.map(|value| serde_json::from_value(value).unwrap());
        assert!(public_review_request(&selected, &config, &selection).is_err(),
            "an incomplete Console permission proposal must not create an access review");
    }
    selection.tool_policy = console_policy("actual_read");
    let request = public_review_request(&selected, &config, &selection).unwrap();
    assert_eq!(request.request_body.unwrap()["application_selection"]["tool_policy"],
        serde_json::to_value(&selection.tool_policy).unwrap());
    // The trusted application body is not the protected Console request.
    selection.tool_policy = None;
    assert!(public_review_body(&selected, &config, &selection).is_ok());
    fixture.owner.retire();
    selection.tool_policy = console_policy("actual_read");
    assert!(public_review_request(&selected, &config, &selection).is_err());
}

#[test]
fn console_service_policy_feedback_uses_exact_checked_exposed_names_and_preserves_proposal() {
    let mut config = McpServerConfig::stdio("fixture", "/unused", vec![], Default::default());
    config.tool_names.insert("raw_read".into(), "visible_read".into());
    let operations = ["raw_read".to_owned(), "unmapped".to_owned()].into();
    let mut policy = console_policy("raw_read");
    let error = require_console_tool_policy(SETUP_PUBLIC_SERVICE_REQUEST, &config, &operations, policy.as_ref()).unwrap_err();
    assert!(matches!(error, ToolError::InvalidArguments { reason, .. }
        if reason.contains("visible_read") && reason.contains("unmapped")
            && reason.contains("has not entered access review")));
    policy = Some(serde_json::from_value(json!({"tools":[
        {"name":"visible_read","consequence":"r1"},{"name":"unmapped","consequence":"r2"}],"catalog":"r0"})).unwrap());
    let retained = policy.clone();
    require_console_tool_policy(SETUP_PUBLIC_SERVICE_REQUEST, &config, &operations, policy.as_ref()).unwrap();
    assert_eq!(policy, retained, "preflight never changes proposed consequences or permissions");
}
