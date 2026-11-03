//! Synthetic canonical-reader observations with the actual selection, account
//! dispatcher, native status owner and protected terminal review. No package
//! execution, OAuth exchange or complete installed CLI journey is established.
use super::*;
use crate::service_install::InstalledConnectorSelection;
use meerkat_auth_core::auth_store::{
    FileLockCoordinator, FileTokenStore, ProviderAuthPersistence, TokenStore,
};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct NoSetupDispatch(AtomicUsize);
#[async_trait]
impl AgentToolDispatcher for NoSetupDispatch {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("retained selection cannot dispatch a package installation")
    }
}
#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
async fn shown(output: &Output, needle: &str) {
    while !output.text().contains(needle) {
        tokio::task::yield_now().await;
    }
}
struct Package {
    storage: ResolvedStorage,
    page: Value,
    readback: Value,
    original: Vec<(PathBuf, Vec<u8>)>,
}
impl Package {
    fn library(root: &Path) -> Self {
        let storage = crate::RealmOptions {
            context_root: root.into(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("home")),
        }
        .resolve("work")
        .unwrap();
        let install = root.join("retained-package");
        let mcp = install.join("ready/mcp.json");
        let receipt = install.join("ready/installed.json");
        let configuration = install.join("configuration/selected.json");
        let request = install.join("original-request.json");
        let document = json!({"mcpServers":{"slack":{"command":install.join("runtime/bin/python"),
            "args":["-I","-m","meerkat_toolkit_services.adapters.services_mcp","serve","--config",configuration]}}});
        let original = vec![
            (mcp.clone(), serde_json::to_vec(&document).unwrap()),
            (
                receipt.clone(),
                b"synthetic canonical installed receipt".to_vec(),
            ),
            (configuration, b"synthetic retained configuration".to_vec()),
            (
                request,
                b"synthetic retained request, never resubmitted".to_vec(),
            ),
        ];
        for (path, bytes) in &original {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let digest = |index: usize| format!("{:x}", Sha256::digest(&original[index].1));
        let locator = json!({"setup":"setup-account-fixture","workspace":format!("work-{}","a".repeat(32)),
            "job_id":"b".repeat(32),"request_sha256":digest(3),"checkpoint_sha256":"c".repeat(64)});
        let readback = json!({"kind":"retained_installation_readback","locator":locator,
            "installation":{"scope":"retained_payload_and_launch_binding_only","family":"services","profile":"slack",
                "job_id":locator["job_id"],"validation_id":"d".repeat(32),"recovery_request_sha256":locator["request_sha256"],
                "mcp_path":mcp,"receipt_path":receipt,"receipt_sha256":digest(1),"configuration_sha256":digest(2),
                "mcp_configuration":document,
                "native_connector":{"schema_version":1,"module":"meerkat_toolkit_services.adapters.services_mcp",
                    "mode":"serve-native","configuration_argument":"--config",
                    "member":"meerkat_toolkit_services/adapters/services_mcp.py","member_sha256":"e".repeat(64),
                    "wheel":{"filename":"meerkat_toolkit_services-0.1.0-py3-none-any.whl","sha256":"f".repeat(64)},
                    "package_sources_sha256":"1".repeat(64)}},
            "metadata":{"kind":"library","selection":{"family":"services","profile":"slack","operations":["auth_test"],"values":{}},
                "help":{"family":"services","profile":"slack","operations":[{"id":"auth_test","tool":"slack_auth_test"}],"fields":[]},
                "configuration_check":{}}});
        let page = json!({"kind":"retained_installation_page","candidates":[{"locator":locator,"family":"services","profile":"slack"}],
            "next_cursor":null,"unconfirmed":false});
        Self {
            storage,
            page,
            readback,
            original,
        }
    }
    fn admit(
        &self,
        owner: &Arc<CheckedInstalls>,
        session: &SessionId,
    ) -> (String, InstalledConnectorSelection) {
        let realm = &self.storage.locator;
        let public = owner
            .retain_candidates(realm, session, self.page.clone())
            .unwrap();
        let handle = public["candidates"][0]["candidate"]
            .as_str()
            .unwrap()
            .to_owned();
        owner
            .retained_select_request(realm, session, &handle)
            .unwrap();
        let verified = owner
            .checked_retained(realm, session, &handle, &self.readback)
            .unwrap();
        let selected = owner
            .admit_retained(realm, session, &handle, verified)
            .unwrap();
        (handle, selected)
    }
    fn unchanged(&self) {
        for (path, bytes) in &self.original {
            assert_eq!(&std::fs::read(path).unwrap(), bytes);
        }
    }
}
fn selection_owner(package: &Package, dispatch: Arc<NoSetupDispatch>) -> Arc<CheckedInstalls> {
    let owner = CheckedInstalls::fixture(dispatch);
    owner.bind_realm(&package.storage.locator).unwrap();
    owner
}
fn secret_metadata(package: &mut Package) {
    package.readback["metadata"]["selection"]["values"] = json!({"token_env":"FIXTURE_TOKEN"});
    package.readback["metadata"]["help"]["fields"] =
        json!([{"name":"token_env","kind":"string","credential_reference":true}]);
    package.readback["metadata"]["configuration_check"]["credential_environment_references"] = json!([
        {"field_path":["config","token_env"],"environment_variable":"FIXTURE_TOKEN"}]);
}

#[test]
fn retained_routes_preserve_independent_evidence_and_never_guess_missing_authentication() {
    for (native, secret, public, coherent, expected) in [
        (true, false, false, true, vec![TOOL]),
        (true, true, false, true, vec![SECRET_TOOL, TOOL]),
        (false, true, false, true, vec![SECRET_TOOL]),
        (false, false, false, true, vec![]),
        (false, false, true, true, vec![PLAIN_TOOL]),
        (true, false, true, true, vec![PLAIN_TOOL, TOOL]),
        (true, false, false, false, vec![]),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut package = Package::library(root.path());
        if !native {
            package.readback["installation"]
                .as_object_mut()
                .unwrap()
                .remove("native_connector");
        }
        if secret {
            secret_metadata(&mut package);
        }
        if public {
            package.readback["metadata"]["help"]["authentication"] = json!({"kind":"none"});
            package.readback["metadata"]["configuration_check"]["credential_environment_references"] =
                json!([]);
        }
        if !coherent {
            package.readback["installation"]["mcp_configuration"]["mcpServers"]["slack"]["args"]
                [3] = json!("other-mode");
        }
        let dispatch = Arc::new(NoSetupDispatch(AtomicUsize::new(0)));
        let owner = selection_owner(&package, dispatch.clone());
        let (_, selected) = package.admit(&owner, &SessionId::new());
        let result = selected_result(&selected);
        assert_eq!(result["available_setup_tools"], json!(expected));
        assert!(result.get("next_tool").is_none());
        assert_eq!(
            result["declared_raw_operations"],
            json!(["slack_auth_test"])
        );
        assert_eq!(
            selected.no_auth(),
            public,
            "route support must not rewrite declared authentication"
        );
        for private in [
            "native_connector",
            "mcp_configuration",
            "mcp_path",
            "receipt_path",
            "configuration_sha256",
            "locator",
        ] {
            assert!(result.get(private).is_none());
        }
        owner.retire();
        assert_eq!(
            selected_result(&selected)["available_setup_tools"],
            json!([])
        );
        assert_eq!(dispatch.0.load(Ordering::SeqCst), 0);
        package.unchanged();
    }
    // An arbitrary process binding is not a native account entry. Only its
    // explicitly declared secret route is available, even with OAuth-like text.
    // Binding fields are toolkit_package::Id values (no spaces).
    let root = tempfile::tempdir().unwrap();
    let mut package = Package::library(root.path());
    package.page["candidates"][0]["family"] = json!("selected-python");
    package.page["candidates"][0]["profile"] = json!("python");
    let declaration = json!({"profile":"python","selection_sha256":"2".repeat(64),"manifest_digest":"3".repeat(64),
        "effect_sha256":"4".repeat(64),"files":{"package.json":"5".repeat(64),"activation.json":"2".repeat(64)},
        "declared_service":{"authentication":{"kind":"binding","owner":"process","requirement":"OAuth-like-custom-binding","credential_audience":"service"},
            "operation_tools":{"read":"slack_auth_test"},"environment_secret_slots":["FIXTURE_TOKEN"]}});
    let installed = package.readback["installation"].as_object_mut().unwrap();
    installed.remove("native_connector");
    installed.insert("family".into(), json!("selected-python"));
    installed.insert("profile".into(), json!("python"));
    installed.insert("selection_kind".into(), json!("python"));
    for field in [
        "selection_sha256",
        "manifest_digest",
        "effect_sha256",
        "declared_service",
    ] {
        installed.insert(field.into(), declaration[field].clone());
    }
    package.readback["metadata"] = json!({"kind":"selected","declaration":declaration});
    let owner = selection_owner(&package, Arc::new(NoSetupDispatch(AtomicUsize::new(0))));
    let (_, selected) = package.admit(&owner, &SessionId::new());
    assert_eq!(
        selected_result(&selected)["available_setup_tools"],
        json!([SECRET_TOOL])
    );
}

fn account_proposal(job: &str) -> Value {
    json!({"job_id":job,"agent":"rkat-agent","expires_in_ms":60000,
        "connector":{"slot_id":"retained-account","issuer":"https://issuer.invalid","client":"explicit-public-client",
            "resource":"https://service.invalid","scopes":["read"],"strategy_id":"oidc-userinfo-v1","account_selection":null},
        "operations":{"slack_auth_test":{"required_scopes":["read"],"destination_origin":"https://service.invalid"}}})
}
fn native_snapshot(storage: &ResolvedStorage) -> Vec<Option<Vec<u8>>> {
    let config = crate::realm::config_source(storage)
        .unwrap()
        .config_doc_path(&storage.locator.realm);
    let realm =
        meerkat_store::realm_paths_in(&storage.locator.state_root, storage.locator.realm.as_str());
    [
        config.clone(),
        config.with_file_name("config_state.json"),
        meerkat_store::grant_owner_store_path(&realm.root),
    ]
    .iter()
    .map(|path| std::fs::read(path).ok())
    .collect()
}

#[tokio::test]
async fn fresh_retained_library_uses_existing_account_dispatch_and_new_protected_review() {
    // Constructing the enclosing future behind a box follows the existing
    // terminal controls; no native setup runtime or package process is opened.
    tokio::time::timeout(
        Duration::from_secs(30),
        Box::pin(account_review_scenarios()),
    )
    .await
    .expect("retained account review must settle without a login exchange");
}
async fn account_review_scenarios() {
    let root = tempfile::tempdir().unwrap();
    let package = Package::library(root.path());
    let dispatch = Arc::new(NoSetupDispatch(AtomicUsize::new(0)));
    let old_owner = selection_owner(&package, dispatch.clone());
    let old_session = SessionId::new();
    let (old_handle, old_selection) = package.admit(&old_owner, &old_session);
    old_owner.retire();
    assert!(old_selection.check_current().is_err());
    drop(old_owner);
    let tokens = Arc::new(FileTokenStore::new(root.path().join("credentials")));
    let persistence = ProviderAuthPersistence::new(
        tokens.clone(),
        Arc::new(FileLockCoordinator::new(root.path().join("locks"))),
    );
    let mut prior_decision: Option<String> = None;
    for invalidate_during_review in [false, true] {
        let session = SessionId::new();
        let installs = selection_owner(&package, dispatch.clone());
        assert!(
            installs
                .retained_select_request(&package.storage.locator, &session, &old_handle)
                .is_err()
        );
        let (handle, selected) = package.admit(&installs, &session);
        plain::recheck_retained(&selected.installed, &selected.configuration, None)
            .await
            .unwrap();
        let result = selected_result(&selected);
        let route = result["available_setup_tools"][0].as_str().unwrap();
        assert_eq!(route, TOOL);
        assert_eq!(result["available_setup_tools"].as_array().unwrap().len(), 1);
        assert!(selected.secret_slots().is_err());
        let mut startup = SessionStartup {
            storage: package.storage.clone(),
            config: meerkat::Config::default(),
            factory: meerkat::AgentFactory::new(root.path().join("factory"))
                .with_provider_auth_persistence(persistence.clone()),
            persistence: None,
            connector_auth: None,
            standalone: None,
        };
        let mut owner = Box::pin(TerminalConnectors::open(&mut startup))
            .await
            .unwrap();
        let before = native_snapshot(&package.storage);
        let tools = owner.dispatcher(installs.clone());
        let context = ToolDispatchContext::default().with_runtime_identity(session.clone(), None);
        let proposal = account_proposal(result["job_id"].as_str().unwrap());
        // Current-session identity and the retained raw ceiling are enforced by
        // the actual account dispatcher before it queues any terminal request.
        let args = serde_json::value::to_raw_value(&proposal).unwrap();
        let foreign =
            ToolDispatchContext::default().with_runtime_identity(old_session.clone(), None);
        assert!(
            tools
                .dispatch_with_context(
                    ToolCallView {
                        id: "foreign",
                        name: route,
                        args: &args
                    },
                    &foreign
                )
                .await
                .is_err()
        );
        let mut expanded = proposal.clone();
        expanded["operations"] = json!({"write_items":{
            "required_scopes":["write"],"destination_origin":"https://service.invalid"}});
        let expanded = serde_json::value::to_raw_value(&expanded).unwrap();
        assert!(
            tools
                .dispatch_with_context(
                    ToolCallView {
                        id: "expanded",
                        name: route,
                        args: &expanded
                    },
                    &context
                )
                .await
                .is_err()
        );
        assert!(owner.requests.try_recv().is_err());
        let mut waiting = Box::pin(tools.dispatch_with_context(
            ToolCallView {
                id: "account",
                name: route,
                args: &args,
            },
            &context,
        ));
        assert!(waiting.as_mut().now_or_never().is_none());
        let request = owner.next().await.unwrap();
        let ConnectorRequest::Account(account) = &request else {
            panic!("the advertised account route must use its existing request")
        };
        assert_eq!(account.installed.job_id, selected.installed.job_id);
        assert_eq!(
            account.selection.original(),
            selected.configuration.as_ref()
        );
        assert_eq!(account.target.issuer, "https://issuer.invalid");
        assert_eq!(account.grant.agent, "rkat-agent");
        assert_eq!(account.grant.expires_in_ms, Some(60000));
        assert!(
            matches!(&account.grant.target,StockGrantTarget::Mcp {server,operations:McpOperations::Exact(names)}
            if server=="slack" && names==&selected.raw_operations)
        );
        let (approvals, mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
        approvals.bind_session(&session).unwrap();
        let (lines, receive) = mpsc::channel(1);
        let mut input = TerminalInput::scripted(receive);
        let seen = Output::default();
        let mut output = seen.clone();
        let human = async {
            shown(&seen, "Client profile [Enter=public default").await;
            // Explicit input chooses the existing public-client option. The
            // routing code never chooses a profile, descriptor or provider.
            lines.send(Ok("".into())).await.unwrap();
            shown(&seen, "Type 'approve ").await;
            let text = seen.text();
            let id = text
                .split("Type 'approve ")
                .last()
                .unwrap()
                .split('\'')
                .next()
                .unwrap()
                .to_owned();
            assert_ne!(prior_decision.as_ref(), Some(&id));
            assert!(
                waiting.as_mut().now_or_never().is_none(),
                "fresh sign-in consent has not been decided"
            );
            for expected in [
                "Sign in for this installed integration",
                "sign_in_only",
                "https://issuer.invalid",
                "explicit-public-client",
                "https://service.invalid",
                "read",
                selected.installed.job_id.as_str(),
            ] {
                assert!(text.contains(expected));
            }
            assert!(!text.contains("Waiting for native sign-in."));
            if invalidate_during_review {
                installs
                    .retained_select_request(&package.storage.locator, &session, &handle)
                    .unwrap();
                assert!(selected.check_current().is_err());
            }
            lines
                .send(Ok(format!(
                    "{} {id}",
                    if invalidate_during_review {
                        "approve"
                    } else {
                        "deny"
                    }
                )))
                .await
                .unwrap();
            id
        };
        let (presented, decision) = tokio::join!(
            owner.present(
                request,
                &approvals,
                &mut presenter,
                &mut input,
                &mut output,
                std::future::pending::<io::Result<()>>()
            ),
            human
        );
        assert!(presented.unwrap());
        let feedback = waiting.await.unwrap();
        assert!(!feedback.result.is_error);
        let feedback: Value = serde_json::from_str(&feedback.result.text_content()).unwrap();
        if invalidate_during_review {
            assert_eq!(feedback["state"], "unavailable");
            assert_eq!(feedback["reason"], "installation_unconfirmed");
        } else {
            assert_eq!(feedback["state"], "denied");
            assert_eq!(feedback["phase"], "sign_in");
        }
        assert_eq!(feedback["grant_issued"], false);
        assert!(!owner.has_reviewed());
        assert!(!seen.text().contains("Waiting for native sign-in."));
        assert!(!seen.text().contains("Credential for "));
        assert!(tokens.list().await.unwrap().is_empty());
        assert_eq!(native_snapshot(&package.storage), before);
        package.unchanged();
        assert_eq!(dispatch.0.load(Ordering::SeqCst), 0);
        input.check_cleanup().unwrap();
        approvals.retire();
        installs.retire();
        owner.retire().unwrap();
        prior_decision = Some(decision);
    }
}
