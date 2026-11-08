//! Synthetic fixed-reader observations; no package, native or live execution.
use super::*;
struct NoDispatch;
#[async_trait]
impl AgentToolDispatcher for NoDispatch {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Arc::from([])
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("retained admission cannot dispatch installation")
    }
}
fn realm() -> RealmLocator {
    let root = tempfile::tempdir().unwrap();
    crate::RealmOptions {
        context_root: root.path().into(),
        state_root: Some(root.path().join("state")),
        user_config_root: Some(root.path().join("home")),
    }
    .resolve("work")
    .unwrap()
    .locator
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
    result["package_sha256"] = declaration["files"]["package.json"].clone();
    result.as_object_mut().unwrap().remove("files");
    result["family"] = json!(family);
    result["selection_kind"] = json!(profile);
    result["job_id"] = locator["job_id"].clone();
    result["validation_id"] = json!("3".repeat(32));
    result["recovery_request_sha256"] = locator["request_sha256"].clone();
    result["scope"] = json!(READBACK_SCOPE);
    result["receipt_sha256"] = json!("4".repeat(64));
    result["mcp_path"] = json!("/fixture/install/ready/mcp.json");
    result["receipt_path"] = json!("/fixture/install/ready/installed.json");
    result["mcp_configuration"] =
        json!({"mcpServers":{"items":{"command":"/fixture/runtime","args":["serve"]}}});
    (
        json!({"kind":"retained_installation_page","candidates":[{"locator":locator,"family":family,"profile":profile}],"next_cursor":null,"unconfirmed":false}),
        json!({"kind":"retained_installation_readback","locator":locator,"installation":result,"metadata":{"kind":"selected","declaration":declaration}}),
    )
}
#[test]
fn fresh_selected_admission_is_session_scoped_and_never_creates_install_authority() {
    for profile in ["python", "node"] {
        let realm = realm();
        let session = SessionId::new();
        let owner = CheckedInstalls::fixture(Arc::new(NoDispatch));
        owner.bind_realm(&realm).unwrap();
        let (page, readback) = fixture(profile);
        let public = owner.retain_candidates(&realm, &session, page).unwrap();
        let key = public["candidates"][0]["candidate"].as_str().unwrap();
        let job = readback["installation"]["job_id"].as_str().unwrap();
        assert!(owner.connector_selection(&realm, &session, job).is_err());
        assert!(
            owner
                .retained_select_request(&realm, &SessionId::new(), key)
                .is_err()
        );
        let mut foreign = realm.clone();
        foreign.state_root = "/other/state".into();
        assert!(
            owner
                .retained_select_request(&foreign, &session, key)
                .is_err()
        );
        owner
            .retained_select_request(&realm, &session, key)
            .unwrap();
        let observed = owner
            .checked_retained(&realm, &session, key, &readback)
            .unwrap();
        let selected = owner
            .admit_retained(&realm, &session, key, observed)
            .unwrap();
        selected.check_current().unwrap();
        assert!(
            selected
                .secret_configuration(&BTreeSet::from(["read_items".into()]))
                .is_ok()
        );
        assert!(
            selected
                .secret_configuration(&BTreeSet::from(["write_items".into()]))
                .is_err()
        );
        let state = owner.state.lock().unwrap();
        assert!(state.proposals.is_empty());
        assert!(state.armed.is_none());
        drop(state);
        owner.retire();
        assert!(selected.check_current().is_err());
        let fresh = CheckedInstalls::fixture(Arc::new(NoDispatch));
        fresh.bind_realm(&realm).unwrap();
        assert!(
            fresh
                .retained_select_request(&realm, &SessionId::new(), key)
                .is_err()
        );
    }
}
#[test]
fn changed_selected_read_invalidates_only_that_candidate_and_does_not_restore_metadata() {
    let realm = realm();
    let session = SessionId::new();
    let owner = CheckedInstalls::fixture(Arc::new(NoDispatch));
    owner.bind_realm(&realm).unwrap();
    let (page, valid) = fixture("python");
    let key = owner
        .retain_candidates(&realm, &session, page.clone())
        .unwrap()["candidates"][0]["candidate"]
        .as_str()
        .unwrap()
        .to_owned();
    owner
        .retained_select_request(&realm, &session, &key)
        .unwrap();
    let selected = owner
        .admit_retained(
            &realm,
            &session,
            &key,
            owner
                .checked_retained(&realm, &session, &key, &valid)
                .unwrap(),
        )
        .unwrap();
    let mut other_page = page;
    other_page["candidates"][0]["locator"]["job_id"] = json!("5".repeat(32));
    let other_key = owner
        .retain_candidates(&realm, &session, other_page)
        .unwrap()["candidates"][0]["candidate"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut other = valid.clone();
    other["locator"]["job_id"] = json!("5".repeat(32));
    other["installation"]["job_id"] = json!("5".repeat(32));
    owner
        .retained_select_request(&realm, &session, &other_key)
        .unwrap();
    let unaffected = owner
        .admit_retained(
            &realm,
            &session,
            &other_key,
            owner
                .checked_retained(&realm, &session, &other_key, &other)
                .unwrap(),
        )
        .unwrap();
    for field in [
        "effect_sha256",
        "declared_service",
        "selection_sha256",
        "recovery_request_sha256",
    ] {
        owner
            .retained_select_request(&realm, &session, &key)
            .unwrap();
        let mut changed = valid.clone();
        changed["installation"][field] = json!("changed");
        assert!(
            owner
                .checked_retained(&realm, &session, &key, &changed)
                .is_err()
        );
        assert!(selected.check_current().is_err());
        unaffected.check_current().unwrap();
    }
    owner
        .retained_select_request(&realm, &session, &key)
        .unwrap();
    assert!(
        owner
            .checked_retained(&realm, &session, &key, &json!({"state":"unavailable"}))
            .is_err()
    );
    unaffected.check_current().unwrap();
    let restored = owner
        .admit_retained(
            &realm,
            &session,
            &key,
            owner
                .checked_retained(&realm, &session, &key, &valid)
                .unwrap(),
        )
        .unwrap();
    restored.check_current().unwrap();
    unaffected.check_current().unwrap();
}

fn official_fixture() -> (Value, Value) {
    let (mut page, mut readback) = fixture("python");
    page["candidates"][0]["family"] = json!("official-github");
    page["candidates"][0]["profile"] = json!("selected-reads");
    let result = &mut readback["installation"];
    result["family"] = json!("official-github");
    result["profile"] = json!("selected-reads");
    result["configuration_sha256"] = json!("6".repeat(64));
    result["mcp_configuration"] = json!({"mcpServers":{"github":{"command":"/fixture/install/github-mcp-server",
        "args":["stdio","--read-only","--gh-host","https://github.com","--tools","get_me,list_commits,get_file_contents"],
        "cwd":"/fixture/install","env":{}}}});
    readback["metadata"] = json!({"kind":"library",
        "selection":{"family":"official-github","profile":"selected-reads",
            "operations":["get_me","list_commits","get_file_contents"],"values":{"token_env":"GITHUB_PERSONAL_ACCESS_TOKEN"}},
        "help":{"family":"official-github","profile":"selected-reads",
            "operations":[{"id":"get_me","tool":"get_me"},{"id":"list_commits","tool":"list_commits"},
                {"id":"get_file_contents","tool":"get_file_contents"}],
            "fields":[{"name":"token_env","kind":"string","credential_reference":true}]},
        "configuration_check":{"credential_environment_references":[{
            "field_path":["config","token_env"],"environment_variable":"GITHUB_PERSONAL_ACCESS_TOKEN"}]}});
    (page, readback)
}

#[test]
fn official_retained_selection_keeps_checked_launch_and_exact_credential_slot() {
    let realm = realm();
    let session = SessionId::new();
    let owner = CheckedInstalls::fixture(Arc::new(NoDispatch));
    owner.bind_realm(&realm).unwrap();
    let (page, readback) = official_fixture();
    let public = owner.retain_candidates(&realm, &session, page).unwrap();
    let key = public["candidates"][0]["candidate"].as_str().unwrap();
    owner
        .retained_select_request(&realm, &session, key)
        .unwrap();
    let verified = owner
        .checked_retained(&realm, &session, key, &readback)
        .unwrap();
    let selected = owner
        .admit_retained(&realm, &session, key, verified)
        .unwrap();
    assert_eq!(
        selected.configuration.as_ref(),
        &readback["installation"]["mcp_configuration"]
    );
    assert_eq!(
        selected.raw_operations,
        BTreeSet::from([
            "get_me".into(),
            "list_commits".into(),
            "get_file_contents".into()
        ])
    );
    let slots = selected.secret_slots().unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(
        slots[0].environment_variable(),
        "GITHUB_PERSONAL_ACCESS_TOKEN"
    );
    assert!(!selected.no_auth());
    assert!(
        selected
            .public_configuration(&BTreeSet::from(["get_me".into()]))
            .is_err()
    );
    assert_eq!(
        selected
            .secret_configuration(&BTreeSet::from(["create_issue".into()]))
            .unwrap_err(),
        "checked_credential_references_required"
    );
    // Project the checked launch and credential names, without starting a
    // server or claiming authentication, attachment or native readiness.
    let config = selected
        .secret_configuration(&BTreeSet::from(["get_me".into()]))
        .unwrap();
    let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &config.transport else {
        panic!("stdio")
    };
    let expected = &readback["installation"]["mcp_configuration"]["mcpServers"]["github"];
    assert_eq!(launch.command, expected["command"].as_str().unwrap());
    assert_eq!(
        serde_json::to_value(&launch.args).unwrap(),
        expected["args"]
    );
    assert_eq!(launch.cwd.as_deref(), expected["cwd"].as_str());
    assert!(launch.env.is_empty() && config.connector.is_none());
    assert_eq!(
        launch.secret_env,
        BTreeSet::from(["GITHUB_PERSONAL_ACCESS_TOKEN".into()])
    );
    assert_eq!(
        config.access.as_ref().unwrap().allowed_operations,
        BTreeSet::from(["get_me".into()])
    );
    let serialized = serde_json::to_value(&config).unwrap();
    assert_eq!(serialized["cwd"], expected["cwd"]);
    assert_eq!(
        serde_json::from_value::<meerkat_core::McpServerConfig>(serialized).unwrap(),
        config
    );
    for cwd in ["", "relative/directory", "/invalid\0directory"] {
        let mut changed = selected.configuration.as_ref().clone();
        changed["mcpServers"]["github"]["cwd"] = json!(cwd);
        assert_eq!(
            super::super::secret_launch_configuration(
                &selected.installed,
                &changed,
                &BTreeSet::from(["get_me".into()]),
                &slots
            )
            .unwrap_err(),
            "unsupported_public_launch"
        );
    }
}

#[test]
fn official_retained_metadata_refuses_alternate_family_profile_names_and_credentials() {
    let realm = realm();
    let session = SessionId::new();
    let owner = CheckedInstalls::fixture(Arc::new(NoDispatch));
    owner.bind_realm(&realm).unwrap();
    let (page, valid) = official_fixture();
    for (family, profile) in [
        ("official-playwright", "selected-reads"),
        ("official-github", "other"),
    ] {
        let mut wrong = page.clone();
        wrong["candidates"][0]["family"] = json!(family);
        wrong["candidates"][0]["profile"] = json!(profile);
        assert!(owner.retain_candidates(&realm, &session, wrong).is_err());
    }
    let public = owner.retain_candidates(&realm, &session, page).unwrap();
    let key = public["candidates"][0]["candidate"].as_str().unwrap();
    owner
        .retained_select_request(&realm, &session, key)
        .unwrap();
    for (pointer, value) in [
        ("/metadata/selection/profile", json!("other")),
        ("/metadata/selection/operations", json!(["get_me"])),
        ("/metadata/selection/values/token_env", json!("OTHER_TOKEN")),
        ("/metadata/help/operations/0/tool", json!("create_issue")),
        ("/metadata/help/fields/0/credential_reference", json!(false)),
        (
            "/metadata/configuration_check/credential_environment_references",
            json!([]),
        ),
        (
            "/metadata/configuration_check/credential_environment_references/0/field_path",
            json!(["token_env"]),
        ),
    ] {
        let mut wrong = valid.clone();
        *wrong.pointer_mut(pointer).unwrap() = value;
        assert!(
            owner
                .checked_retained(&realm, &session, key, &wrong)
                .is_err(),
            "{pointer}"
        );
    }
    assert!(
        owner
            .checked_retained(&realm, &session, key, &valid)
            .is_ok()
    );
}

fn enterprise_fixture(operations: &[&str]) -> (Value, Value) {
    let (mut page, mut readback) = official_fixture();
    page["candidates"][0]["profile"] = json!("enterprise-selected-reads");
    readback["installation"]["profile"] = json!("enterprise-selected-reads");
    readback["installation"]["mcp_configuration"]["mcpServers"]["github"]["args"] = json!([
        "stdio",
        "--read-only",
        "--gh-host",
        "https://github.enterprise.example",
        "--toolsets=",
        "--tools",
        operations.join(",")
    ]);
    readback["metadata"] = json!({"kind":"library",
        "selection":{"family":"official-github","profile":"enterprise-selected-reads",
            "operations":operations,"values":{"token_env":"GITHUB_PERSONAL_ACCESS_TOKEN"}},
        "help":{"family":"official-github","profile":"enterprise-selected-reads",
            "operations":[{"id":"list_pull_requests","tool":"list_pull_requests"},
                {"id":"pull_request_read","tool":"pull_request_read"},
                {"id":"list_commits","tool":"list_commits"},{"id":"get_commit","tool":"get_commit"},
                {"id":"actions_list","tool":"actions_list"},{"id":"actions_get","tool":"actions_get"},
                {"id":"list_issues","tool":"list_issues"},{"id":"issue_read","tool":"issue_read"},
                {"id":"get_me","tool":"get_me"}],
            "fields":[{"name":"token_env","kind":"string","credential_reference":true},
                {"name":"github_host","kind":"string","credential_reference":false}]},
        "configuration_check":{"credential_environment_references":[{
            "field_path":["config","token_env"],"environment_variable":"GITHUB_PERSONAL_ACCESS_TOKEN"}]}});
    (page, readback)
}

#[test]
fn enterprise_retained_selection_preserves_subset_launch_and_only_token_reference() {
    for operations in [
        vec!["list_pull_requests"],
        vec!["issue_read"],
        vec!["get_me"],
        vec!["pull_request_read", "get_commit", "actions_get"],
        vec![
            "list_pull_requests",
            "pull_request_read",
            "list_commits",
            "get_commit",
            "actions_list",
            "actions_get",
            "list_issues",
            "issue_read",
            "get_me",
        ],
    ] {
        let realm = realm();
        let session = SessionId::new();
        let owner = CheckedInstalls::fixture(Arc::new(NoDispatch));
        owner.bind_realm(&realm).unwrap();
        let (page, readback) = enterprise_fixture(&operations);
        let public = owner.retain_candidates(&realm, &session, page).unwrap();
        let key = public["candidates"][0]["candidate"].as_str().unwrap();
        owner
            .retained_select_request(&realm, &session, key)
            .unwrap();
        let verified = owner
            .checked_retained(&realm, &session, key, &readback)
            .unwrap();
        let selected = owner
            .admit_retained(&realm, &session, key, verified)
            .unwrap();
        let expected = operations
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(selected.raw_operations, expected);
        assert_eq!(
            selected.configuration.as_ref(),
            &readback["installation"]["mcp_configuration"]
        );
        assert_eq!(selected.installed.profile, "enterprise-selected-reads");
        assert_eq!(
            selected.installed.configuration_sha256,
            Some("6".repeat(64))
        );
        assert_eq!(
            selected
                .secret_slots()
                .unwrap()
                .iter()
                .map(|slot| slot.environment_variable().to_owned())
                .collect::<Vec<_>>(),
            vec!["GITHUB_PERSONAL_ACCESS_TOKEN".to_owned()]
        );
        assert!(selected.public_configuration(&expected).is_err());
        for outside in ["get_me", "get_file_contents", "create_issue"]
            .into_iter()
            .filter(|name| !expected.contains(*name))
        {
            assert!(
                selected
                    .secret_configuration(&BTreeSet::from([outside.to_owned()]))
                    .is_err()
            );
        }
        let config = selected.secret_configuration(&expected).unwrap();
        let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &config.transport else {
            panic!("stdio")
        };
        assert_eq!(
            serde_json::to_value(&launch.args).unwrap(),
            readback["installation"]["mcp_configuration"]["mcpServers"]["github"]["args"]
        );
        assert_eq!(launch.args[4], "--toolsets=");
        assert_eq!(
            launch.secret_env,
            BTreeSet::from(["GITHUB_PERSONAL_ACCESS_TOKEN".into()])
        );
        assert!(launch.env.is_empty() && config.connector.is_none());
        assert_eq!(config.access.as_ref().unwrap().allowed_operations, expected);
        assert_eq!(
            readback["metadata"]["selection"]["values"],
            json!({"token_env":"GITHUB_PERSONAL_ACCESS_TOKEN"})
        );
        assert!(owner.state.lock().unwrap().proposals.is_empty());
    }
}

#[test]
fn enterprise_identity_catalog_preserves_exact_earlier_retained_metadata() {
    let (_, mut readback) = enterprise_fixture(&["issue_read"]);
    readback["metadata"]["help"]["operations"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(checked_enterprise_metadata(&readback["metadata"]).is_ok());
    readback["metadata"]["selection"]["operations"] = json!(["get_me"]);
    assert!(checked_enterprise_metadata(&readback["metadata"]).is_err());
    readback["metadata"]["help"]["operations"][7] = json!({"id":"get_me","tool":"get_me"});
    assert!(checked_enterprise_metadata(&readback["metadata"]).is_err());
}

#[test]
fn enterprise_retained_metadata_refuses_reordering_cross_profile_and_host_or_secret_values() {
    let realm = realm();
    let session = SessionId::new();
    let owner = CheckedInstalls::fixture(Arc::new(NoDispatch));
    owner.bind_realm(&realm).unwrap();
    let (page, valid) = enterprise_fixture(&["list_pull_requests", "get_commit", "issue_read"]);
    let public = owner.retain_candidates(&realm, &session, page).unwrap();
    let key = public["candidates"][0]["candidate"].as_str().unwrap();
    owner
        .retained_select_request(&realm, &session, key)
        .unwrap();
    // get_me is an explicit Enterprise read; get_file_contents remains outside this catalog.
    for operations in [
        json!([]),
        json!(["get_commit", "get_commit"]),
        json!(["issue_read", "get_commit"]),
        json!(["get_file_contents"]),
        json!(["create_issue"]),
        json!([null]),
        json!("get_commit"),
    ] {
        let mut wrong = valid.clone();
        wrong["metadata"]["selection"]["operations"] = operations;
        assert!(
            owner
                .checked_retained(&realm, &session, key, &wrong)
                .is_err(),
            "operations: {}",
            wrong["metadata"]["selection"]["operations"]
        );
    }
    for (pointer, value) in [
        ("/metadata/selection/profile", json!("selected-reads")),
        ("/metadata/help/profile", json!("selected-reads")),
        ("/installation/profile", json!("selected-reads")),
        (
            "/metadata/selection/values",
            json!({"token_env":"GITHUB_PERSONAL_ACCESS_TOKEN", "github_host":"https://github.enterprise.example"}),
        ),
        (
            "/metadata/selection/values",
            json!({"token_env":"GITHUB_PERSONAL_ACCESS_TOKEN", "token":"fixture-secret-not-a-reference"}),
        ),
        ("/metadata/selection/values/token_env", json!("OTHER_TOKEN")),
        ("/metadata/help/operations/0/tool", json!("create_issue")),
        ("/metadata/help/fields/0/credential_reference", json!(false)),
        ("/metadata/help/fields/1/credential_reference", json!(true)),
        ("/metadata/help/fields/1/kind", json!("url")),
        (
            "/metadata/configuration_check/credential_environment_references",
            json!([]),
        ),
        (
            "/metadata/configuration_check/credential_environment_references/0/environment_variable",
            json!("OTHER_TOKEN"),
        ),
        ("/installation/configuration_sha256", Value::Null),
    ] {
        let mut wrong = valid.clone();
        *wrong.pointer_mut(pointer).unwrap() = value;
        assert!(
            owner
                .checked_retained(&realm, &session, key, &wrong)
                .is_err(),
            "{pointer}"
        );
    }
    for field in ["fields", "operations"] {
        let mut wrong = valid.clone();
        wrong["metadata"]["help"][field]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        assert!(
            owner
                .checked_retained(&realm, &session, key, &wrong)
                .is_err(),
            "{field}"
        );
    }
    assert!(
        owner
            .checked_retained(&realm, &session, key, &valid)
            .is_ok()
    );
    let (_, mut old) = official_fixture();
    old["metadata"]["help"] = valid["metadata"]["help"].clone();
    assert!(checked_official_metadata(&old["metadata"]).is_err());
}
