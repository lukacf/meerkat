//! Synthetic fixed-reader observations; no package, native or live execution.
use super::*;
struct NoDispatch;
#[async_trait]
impl AgentToolDispatcher for NoDispatch {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> { Arc::from([]) }
    async fn dispatch(&self, _:ToolCallView<'_>) -> Result<ToolDispatchOutcome,ToolError> { panic!("retained admission cannot dispatch installation") }
}
fn realm() -> RealmLocator {
    let root=tempfile::tempdir().unwrap();
    crate::RealmOptions {context_root:root.path().into(),state_root:Some(root.path().join("state")),user_config_root:Some(root.path().join("home"))}
        .resolve("work").unwrap().locator
}
fn fixture(profile: &str) -> (Value, Value) {
    let locator = json!({"setup":"setup-fixture","workspace":format!("work-{}","a".repeat(32)),"job_id":"b".repeat(32),
        "request_sha256":"c".repeat(64),"checkpoint_sha256":"d".repeat(64)});
    let family = format!("selected-{profile}");
    let declaration = json!({"profile":profile,"selection_sha256":"e".repeat(64),"manifest_digest":"f".repeat(64),"effect_sha256":"1".repeat(64),
        "files":{"package.json":"2".repeat(64),"activation.json":"e".repeat(64)},
        "declared_service":{"authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
            "operation_tools":{"read":"read_items"},"environment_secret_slots":["SERVICE_KEY"]}});
    let mut result = declaration.clone();
    result["package_sha256"]=declaration["files"]["package.json"].clone();
    result.as_object_mut().unwrap().remove("files");
    result["family"]=json!(family); result["selection_kind"]=json!(profile);
    result["job_id"]=locator["job_id"].clone();result["validation_id"]=json!("3".repeat(32));
    result["recovery_request_sha256"]=locator["request_sha256"].clone();
    result["scope"]=json!(READBACK_SCOPE);result["receipt_sha256"]=json!("4".repeat(64));
    result["mcp_path"]=json!("/fixture/install/ready/mcp.json");result["receipt_path"]=json!("/fixture/install/ready/installed.json");
    result["mcp_configuration"]=json!({"mcpServers":{"items":{"command":"/fixture/runtime","args":["serve"]}}});
    (json!({"kind":"retained_installation_page","candidates":[{"locator":locator,"family":family,"profile":profile}],"next_cursor":null,"unconfirmed":false}),
     json!({"kind":"retained_installation_readback","locator":locator,"installation":result,"metadata":{"kind":"selected","declaration":declaration}}))
}
#[test]
fn fresh_selected_admission_is_session_scoped_and_never_creates_install_authority() {
    for profile in ["python","node"] {
        let realm=realm();let session=SessionId::new();let owner=CheckedInstalls::fixture(Arc::new(NoDispatch));
        owner.bind_realm(&realm).unwrap();let (page, readback)=fixture(profile);
        let public=owner.retain_candidates(&realm,&session,page).unwrap();
        let key=public["candidates"][0]["candidate"].as_str().unwrap();
        let job=readback["installation"]["job_id"].as_str().unwrap();
        assert!(owner.connector_selection(&realm,&session,job).is_err());
        assert!(owner.retained_select_request(&realm,&SessionId::new(),key).is_err());
        let mut foreign=realm.clone();foreign.state_root="/other/state".into();
        assert!(owner.retained_select_request(&foreign,&session,key).is_err());
        owner.retained_select_request(&realm,&session,key).unwrap();
        let observed=owner.checked_retained(&realm,&session,key,&readback).unwrap();
        let selected=owner.admit_retained(&realm,&session,key,observed).unwrap();
        selected.check_current().unwrap();
        assert!(selected.secret_configuration(&BTreeSet::from(["read_items".into()])).is_ok());
        assert!(selected.secret_configuration(&BTreeSet::from(["write_items".into()])).is_err());
        let state=owner.state.lock().unwrap();assert!(state.proposals.is_empty());assert!(state.armed.is_none());drop(state);
        owner.retire();assert!(selected.check_current().is_err());
        let fresh=CheckedInstalls::fixture(Arc::new(NoDispatch));fresh.bind_realm(&realm).unwrap();
        assert!(fresh.retained_select_request(&realm,&SessionId::new(),key).is_err());
    }
}
#[test]
fn changed_selected_read_invalidates_only_that_candidate_and_does_not_restore_metadata() {
    let realm=realm();let session=SessionId::new();let owner=CheckedInstalls::fixture(Arc::new(NoDispatch));owner.bind_realm(&realm).unwrap();
    let (page, valid)=fixture("python");
    let key=owner.retain_candidates(&realm,&session,page.clone()).unwrap()["candidates"][0]["candidate"].as_str().unwrap().to_owned();
    owner.retained_select_request(&realm,&session,&key).unwrap();
    let selected=owner.admit_retained(&realm,&session,&key,owner.checked_retained(&realm,&session,&key,&valid).unwrap()).unwrap();
    let mut other_page=page;other_page["candidates"][0]["locator"]["job_id"]=json!("5".repeat(32));
    let other_key=owner.retain_candidates(&realm,&session,other_page).unwrap()["candidates"][0]["candidate"].as_str().unwrap().to_owned();
    let mut other=valid.clone();other["locator"]["job_id"]=json!("5".repeat(32));other["installation"]["job_id"]=json!("5".repeat(32));
    owner.retained_select_request(&realm,&session,&other_key).unwrap();
    let unaffected=owner.admit_retained(&realm,&session,&other_key,owner.checked_retained(&realm,&session,&other_key,&other).unwrap()).unwrap();
    for field in ["effect_sha256","declared_service","selection_sha256","recovery_request_sha256"] {
        owner.retained_select_request(&realm,&session,&key).unwrap();
        let mut changed=valid.clone();changed["installation"][field]=json!("changed");
        assert!(owner.checked_retained(&realm,&session,&key,&changed).is_err());
        assert!(selected.check_current().is_err());unaffected.check_current().unwrap();
    }
    owner.retained_select_request(&realm,&session,&key).unwrap();
    assert!(owner.checked_retained(&realm,&session,&key,&json!({"state":"unavailable"})).is_err());
    unaffected.check_current().unwrap();
    let restored=owner.admit_retained(&realm,&session,&key,owner.checked_retained(&realm,&session,&key,&valid).unwrap()).unwrap();
    restored.check_current().unwrap();unaffected.check_current().unwrap();
}
