//! Response-shape controls only. These do not execute an MCP server or install.
use super::*;
use meerkat_core::{ToolResult, ops::{AsyncOpRef, OperationId, SessionEffect}};
use serde_json::value::RawValue;

fn structured(raw: &str) -> ContentBlock {
    ContentBlock::Structured { data: RawValue::from_string(raw.into()).unwrap() }
}
fn outcome(content: Vec<ContentBlock>) -> ToolDispatchOutcome {
    ToolResult::with_blocks("check".into(), content, false).into()
}
fn read(outcome: &ToolDispatchOutcome) -> Result<Value, ToolError> {
    let args = RawValue::from_string("{}".into()).unwrap();
    result_document(ToolCallView { id: "check", name: "setup_check", args: &args }, outcome)
}

#[test]
fn single_structured_and_text_documents_share_exact_object_and_byte_bounds() {
    let raw = r#"{"scope":"configuration_only","nested":{"value":1}}"#;
    let expected: Value = serde_json::from_str(raw).unwrap();
    for content in [ContentBlock::Text { text: raw.into() }, structured(raw)] {
        assert_eq!(read(&outcome(vec![content])).unwrap(), expected);
    }
    let overhead = r#"{"payload":""}"#.len();
    let exact = format!(r#"{{"payload":"{}"}}"#, "a".repeat(MAX_DOCUMENT - overhead));
    assert_eq!(exact.len(), MAX_DOCUMENT);
    for content in [ContentBlock::Text { text: exact.clone() }, structured(&exact)] {
        assert_eq!(read(&outcome(vec![content])).unwrap()["payload"].as_str().unwrap().len(),
            MAX_DOCUMENT - overhead);
    }
    let oversized = format!(r#"{{"payload":"{}"}}"#, "a".repeat(MAX_DOCUMENT - overhead + 1));
    for content in [ContentBlock::Text { text: oversized.clone() }, structured(&oversized)] {
        assert!(read(&outcome(vec![content])).is_err());
    }
}

#[test]
fn structured_documents_refuse_ambiguous_content_duplicate_keys_and_non_objects() {
    let text = || ContentBlock::Text { text: "{}".into() };
    for content in [
        vec![],
        vec![text(), text()],
        vec![structured("{}"), structured("{}")],
        vec![text(), structured("{}")],
        vec![structured("{}"), text()],
        vec![text(), structured(r#"{"different":true}"#)],
    ] {
        assert!(read(&outcome(content)).is_err());
    }
    for raw in ["null", "[]", "1", "true", r#""object as text""#,
        r#"{"scope":"configuration_only","scope":"other"}"#,
        r#"{"nested":{"value":1,"value":2}}"#] {
        for content in [ContentBlock::Text { text: raw.into() }, structured(raw)] {
            assert!(read(&outcome(vec![content])).is_err());
        }
    }
    assert!(read(&outcome(vec![ContentBlock::Text { text: "{broken".into() }])).is_err());
}

#[test]
fn structured_document_keeps_exact_call_error_async_and_session_effect_guards() {
    let clean = || outcome(vec![structured(r#"{"scope":"configuration_only"}"#)]);
    let mut wrong_call = clean();
    wrong_call.result.tool_use_id = "another-call".into();
    assert!(read(&wrong_call).is_err());
    let mut error = clean();
    error.result.is_error = true;
    assert!(read(&error).is_err());
    for asynchronous in [AsyncOpRef::barrier(OperationId::new()),
        AsyncOpRef::detached(OperationId::new())] {
        let mut pending = clean();
        pending.async_ops.push(asynchronous);
        assert!(read(&pending).is_err());
    }
    let mut effect = clean();
    effect.session_effects.push(SessionEffect::AppendAssistantBlocks { blocks: vec![] });
    assert!(read(&effect).is_err());
    assert!(read(&clean()).is_ok());
}

#[test]
fn owner_error_documents_keep_call_effect_and_json_guards() {
    let args = RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView { id: "check", name: "setup_reconcile", args: &args };
    let error = |content| {
        let mut reply = outcome(content);
        reply.result.is_error = true;
        reply
    };
    let raw = r#"{"error":"operation_in_progress"}"#;
    for content in [ContentBlock::Text { text: raw.into() }, structured(raw)] {
        let reply = error(vec![content]);
        assert_eq!(response_document(call, &reply).unwrap(), json!({"error":"operation_in_progress"}));
        assert!(result_document(call, &reply).is_err());
    }
    let clean = || error(vec![structured(raw)]);
    let mut wrong_call = clean();
    wrong_call.result.tool_use_id = "another-call".into();
    assert!(response_document(call, &wrong_call).is_err());
    for asynchronous in [AsyncOpRef::barrier(OperationId::new()),
        AsyncOpRef::detached(OperationId::new())] {
        let mut pending = clean();
        pending.async_ops.push(asynchronous);
        assert!(response_document(call, &pending).is_err());
    }
    let mut effect = clean();
    effect.session_effects.push(SessionEffect::AppendAssistantBlocks { blocks: vec![] });
    assert!(response_document(call, &effect).is_err());
    for content in [vec![], vec![structured(raw), structured(raw)],
        vec![structured(r#"{"error":"busy","error":"changed"}"#)],
        vec![structured("null")],
        vec![ContentBlock::Text { text: "{broken".into() }],
        vec![structured(&format!(r#"{{"error":"{}"}}"#, "x".repeat(MAX_DOCUMENT)))]] {
        assert!(response_document(call, &error(content)).is_err());
    }
}

#[test]
fn secret_references_keep_the_exact_checked_field_and_environment_mapping() {
    let help = json!({"fields":[
        {"name":"api_key_env","kind":"string","credential_reference":true},
        {"name":"token_env","kind":"string","credential_reference":true},
        {"name":"timeout","kind":"integer","credential_reference":false}
    ]});
    let metadata = OperationEvidence::Publisher { names:BTreeMap::new(), no_auth:false,
        credential_fields:credential_fields(&help).unwrap() };
    let proposal = json!({"selection":{"family":"agno","values":{
        "api_key_env":"TRELLO_KEY","token_env":"TRELLO_TOKEN","timeout":7}},
        "owner_proposal":{"credential_environment_references":[
            {"field_path":["config","token_env"],"environment_variable":"TRELLO_TOKEN"},
            {"field_path":["config","api_key_env"],"environment_variable":"TRELLO_KEY"}
        ]}});
    let selected = service_credential_references(&proposal, &metadata).unwrap().unwrap();
    assert_eq!(selected[0].field_path, vec!["config".to_owned(),"api_key_env".to_owned()]);
    assert_eq!(selected[0].environment_variable, "TRELLO_KEY");
    assert_eq!(selected[1].environment_variable, "TRELLO_TOKEN");
    for mutation in 0..7 {
        let mut value = proposal.clone();
        let rows = value["owner_proposal"]["credential_environment_references"].as_array_mut().unwrap();
        match mutation {
            0 => { rows.pop(); },
            1 => rows.push(rows[0].clone()),
            2 => rows[0]["field_path"] = json!(["config","timeout"]),
            3 => rows[0]["environment_variable"] = json!("OTHER_TOKEN"),
            4 => rows[0]["environment_variable"] = json!("secret value"),
            5 => rows[0]["extra"] = json!("not accepted"),
            _ => rows[0]["field_path"] = json!(["config.token_env"]),
        }
        assert!(service_credential_references(&value, &metadata).is_err(), "mutation {mutation}");
    }
    let mut shared = proposal.clone();
    shared["selection"]["values"]["token_env"] = json!("TRELLO_KEY");
    shared["owner_proposal"]["credential_environment_references"][0]["environment_variable"] = json!("TRELLO_KEY");
    assert!(service_credential_references(&shared, &metadata).is_err(), "two fields cannot collapse into one slot");
    let mut legacy = proposal;
    legacy["owner_proposal"].as_object_mut().unwrap().remove("credential_environment_references");
    assert!(service_credential_references(&legacy, &metadata).unwrap().is_none());
    assert!(credential_fields(&json!({})).unwrap().is_none());
}

#[test]
fn checked_secret_selection_keeps_normal_launch_with_optional_native_capability() {
    // Only the current checked-record projection is exercised. No package,
    // native grant, credential write or service is executed by this control.
    struct MetadataOnly;
    #[async_trait]
    impl AgentToolDispatcher for MetadataOnly {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> { Vec::new().into() }
        fn execution_binding_epoch(&self, _: &str) -> u64 { 7 }
        async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            panic!("metadata-only control must not dispatch")
        }
    }
    let root = tempfile::tempdir().unwrap();
    let realm = crate::RealmOptions { context_root:root.path().to_owned(),
        state_root:Some(root.path().join("state")),user_config_root:Some(root.path().join("home")) }
        .resolve("metadata-only").unwrap().locator;
    let session = SessionId::new();
    let binding = Arc::new(MetadataOnly);
    let owner = CheckedInstalls::fixture(binding.clone());
    owner.bind_realm(&realm).unwrap();
    let validation_id = "a".repeat(32);
    let job_id = "b".repeat(32);
    let installed = InstalledSelection { realm:realm.clone(),session:session.clone(),job_id:job_id.clone(),
        validation_id:validation_id.clone(),family:"services".into(),profile:"slack".into(),
        mcp_path:root.path().join("ready/mcp.json"),receipt_path:root.path().join("ready/installed.json"),
        receipt_sha256:Some("1".repeat(64)),configuration_sha256:Some("2".repeat(64)),
        server_name:"checked-slack".into(),mcp_document_sha256:"3".repeat(64),recovery_request_sha256:None };
    let normal_args = json!(["-I","-m","meerkat_toolkit_services.adapters.services_mcp",
        "serve","--config",root.path().join("selected.json")]);
    let configuration = Arc::new(json!({"mcpServers":{"checked-slack":{
        "command":"/fixture/installed/python","args":normal_args,"env":{}}}}));
    let capability = NativeConnectorEntry::observed(&json!({
        "schema_version":1,"module":"meerkat_toolkit_services.adapters.services_mcp",
        "mode":"serve-native","configuration_argument":"--config",
        "member":"meerkat_toolkit_services/adapters/services_mcp.py","member_sha256":"4".repeat(64),
        "wheel":{"filename":"meerkat_toolkit_services-1.0.0-py3-none-any.whl","sha256":"5".repeat(64)},
        "package_sources_sha256":"6".repeat(64)
    }),"services","slack").unwrap();
    let proposal = Arc::new(json!({"selection":{"family":"services","profile":"slack",
        "operations":["auth_test"],"values":{"token_env":"CHECKED_SLACK_TOKEN"}},
        "owner_proposal":{"credential_environment_references":[
            {"field_path":["config","token_env"],"environment_variable":"CHECKED_SLACK_TOKEN"}]}}));
    let operation_names = Arc::new(Mutex::new(OperationMetadata { publisher:None,
        observed:Some(OperationEvidence::Publisher { names:BTreeMap::from([("auth_test".into(),"auth_test".into())]),
            no_auth:false,credential_fields:Some(std::collections::BTreeSet::from(["token_env".into()])) }) }));
    let receipt = Arc::new(Mutex::new(ReceiptCapture { installed:Some(installed),
        reconciliation_revision:Some(1),matched:true,configuration:Some(configuration.clone()),
        native_connector:Some(capability),recovery_request_sha256:None }));
    let readback = Readback { epoch:7,session:session.clone(),job:Arc::new(Mutex::new(Some(job_id.clone()))),
        family:"services".into(),binding,realm:realm.clone(),validation_id:validation_id.clone(),
        proposal:proposal.clone(),receipt:receipt.clone(),operation_names:operation_names.clone() };
    owner.state.lock().unwrap().proposals.insert(validation_id,Proposal { session:session.clone(),epoch:7,
        kind:Kind::Library,install:json!({}),review:proposal.as_ref().clone(),used:true,
        readback:Some(readback),operation_names });
    let selected = owner.connector_selection(&realm,&session,&job_id)
        .unwrap_or_else(|_| panic!("exact checked metadata must select"));
    assert!(selected.native_connector.is_some() && selected.check_current().is_ok());
    let config = selected.secret_configuration(&std::collections::BTreeSet::from(["auth_test".into()])).unwrap();
    let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &config.transport else { panic!("stdio") };
    assert_eq!(launch.command,"/fixture/installed/python");
    assert_eq!(serde_json::to_value(&launch.args).unwrap(),normal_args);
    assert_eq!(launch.secret_env,std::collections::BTreeSet::from(["CHECKED_SLACK_TOKEN".into()]));
    assert!(launch.env.is_empty() && config.connector.is_none());
    assert_eq!(config.access.as_ref().unwrap().allowed_operations,
        std::collections::BTreeSet::from(["auth_test".into()]));
    receipt.lock().unwrap().native_connector = None;
    assert!(selected.secret_configuration(&std::collections::BTreeSet::from(["auth_test".into()])).is_err());
}

fn selected_secret_proposal(profile: &str, software: bool) -> Value {
    let mut proposal = json!({"profile":profile,"declared_service":{
        "authentication":{"kind":"binding","owner":"process","requirement":"api-key","credential_audience":"service"},
        "operation_tools":{"read":"actual_read"},"environment_secret_slots":["API_KEY","TOKEN"]},
        "selection_sha256":"a".repeat(64),"effect_sha256":"b".repeat(64),
        "files":{"package.json":"c".repeat(64)}});
    if software {
        proposal["document_kind"] = json!("local_software_declaration");
        proposal["declaration_digest"] = json!("d".repeat(64));
        proposal["files"]["declaration.json"] = json!("a".repeat(64));
    } else {
        proposal["manifest_digest"] = json!("d".repeat(64));
        proposal["files"]["activation.json"] = json!("a".repeat(64));
    }
    proposal
}

#[test]
fn selected_secret_metadata_preserves_source_and_refuses_malformed_or_downgraded_slots() {
    for profile in ["python","node"] { for software in [false,true] {
        let proposal = selected_secret_proposal(profile,software);
        let metadata = selected_service_metadata(&proposal).unwrap().unwrap();
        let OperationEvidence::Selected { inputs, .. } = &metadata else { panic!("selected source") };
        assert_eq!(inputs.environment_secret_slots.as_ref().unwrap().names(), &["API_KEY".to_owned(),"TOKEN".to_owned()]);
        assert_eq!(inputs.document.filename(),if software {"declaration.json"} else {"activation.json"});
        assert!(service_credential_references(&json!({"owner_proposal":proposal.clone()}),&metadata).unwrap().is_none(),
            "a selected environment declaration never becomes a library field mapping");
        let mut legacy = proposal.clone();
        legacy["declared_service"].as_object_mut().unwrap().remove("environment_secret_slots");
        assert!(selected_service_metadata(&legacy).unwrap().is_some());
        for slots in [Value::Null,json!([]),json!(["TOKEN","TOKEN"]),json!(["bad name"]),json!(["TOKEN","API_KEY"])] {
            let mut changed=proposal.clone();changed["declared_service"]["environment_secret_slots"]=slots;
            assert!(selected_service_metadata(&changed).is_err());
        }
        let mut public=proposal.clone();public["declared_service"]["authentication"]=json!({"kind":"none"});
        assert!(selected_service_metadata(&public).is_err());
        let mut other=proposal.clone();other["profile"]=json!("mcpb");assert!(selected_service_metadata(&other).is_err());
        let mut document=proposal;document["files"][inputs.document.filename()]=json!("0".repeat(64));
        assert!(selected_service_metadata(&document).is_err());
    }}
}


#[test]
fn public_operation_feedback_never_accepts_ambiguous_selected_metadata() {
    for software in [false,true] {
        let mut proposal=selected_secret_proposal("python",software);
        proposal["declared_service"]["authentication"]=json!({"kind":"none"});
        proposal["declared_service"].as_object_mut().unwrap().remove("environment_secret_slots");
        proposal["declared_service"]["operation_tools"]=json!({"read-item":"actual_read","list-items":"actual_list"});
        let metadata=selected_service_metadata(&proposal).unwrap().unwrap();
        assert_eq!(metadata.names().get("read-item").map(String::as_str),Some("actual_read"));
        for names in [json!({"read-item":"actual_read","list-items":"actual_read"}),
            json!({"read-item":""}),json!({})] {
            proposal["declared_service"]["operation_tools"]=names;
            assert!(selected_service_metadata(&proposal).is_err());
        }
    }
}
