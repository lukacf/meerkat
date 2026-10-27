//! The full pre-stage consumer over the actual file store and native config
//! writer. The injected failure is after document persistence, before the
//! generation rename. No native grant, controller, adapter or model is mocked.

use super::*;
use meerkat_core::{Config, ConfigDelta, ConfigError, ConfigRuntime, ConfigRuntimeError,
    ConfigStore, ConfigWarning, FileConfigStore};
use std::sync::atomic::{AtomicUsize, Ordering};

struct ObstructGenerationAfterPatch {
    file: FileConfigStore,
    state_path: PathBuf,
    patched: AtomicUsize,
}

#[async_trait::async_trait]
impl ConfigStore for ObstructGenerationAfterPatch {
    async fn get(&self) -> Result<Config, ConfigError> { self.file.get().await }

    async fn set(&self, config: Config) -> Result<(), ConfigError> { self.file.set(config).await }

    async fn patch_preview(&self, delta: &ConfigDelta) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        self.file.patch_preview(delta).await
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let config = self.file.patch(delta).await?;
        self.patched.fetch_add(1, Ordering::SeqCst);
        // The selected sidecar was absent at review. A directory at that
        // exact path makes the native writer's later rename fail, after the
        // real FileConfigStore has already published its document.
        tokio::fs::create_dir(&self.state_path).await?;
        Ok(config)
    }
}

struct ApproveRenderedReview {
    bytes: Vec<u8>,
    input: tokio::sync::mpsc::Sender<io::Result<String>>,
    decisions: usize,
}

impl Write for ApproveRenderedReview {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.decisions == 0 {
            let text = std::str::from_utf8(&self.bytes).expect("rendered terminal text");
            if let Some((_, decision)) = text.split_once("Type 'approve ") {
                let (id, _) = decision.split_once('\'').expect("actual displayed approval ID");
                self.input.try_send(Ok(format!("approve {id}"))).expect("one exact protected decision");
                self.decisions += 1;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn partial_config_publication_returns_before_reviewed_grant_with_real_owner_unchanged() {
    tokio::time::timeout(std::time::Duration::from_secs(30), partial_config_publication_scenario())
        .await
        .expect("the complete protected-review and partial-publication scenario must settle");
}

async fn partial_config_publication_scenario() {
    use meerkat::governed_realm::stock::{StockPrincipals, govern_stock_realm};

    let directory = tempfile::tempdir().unwrap();
    let storage = crate::RealmOptions {
        context_root: directory.path().to_owned(),
        state_root: Some(directory.path().join("state")),
        user_config_root: Some(directory.path().join("home")),
    }.resolve("work").unwrap();
    let source = crate::realm::config_source(&storage).unwrap();
    let config_path = source.config_doc_path(&storage.locator.realm);
    let state_path = config_path.with_file_name("config_state.json");
    let global_path = storage.layout.user_rkat_root().unwrap().join("config.toml");
    let server = |name: &str, arg: &str| McpServerConfig::stdio(name, "/bin/true", vec![arg.into()], Default::default());
    let old_helper = server(crate::setup::SERVER_NAME, "prior-helper");
    let unrelated = server("unrelated", "keep-this-entry");
    let parent = FileConfigStore::new(global_path.clone(), meerkat_models::canonical());
    parent.patch(ConfigDelta(serde_json::json!({"max_tokens": 9999,
        "tools":{"mcp_servers":[server("inherited", "parent-only")]}}))).await.unwrap();
    let file = FileConfigStore::new(config_path.clone(), meerkat_models::canonical());
    file.patch(ConfigDelta(serde_json::json!({"max_tokens":1234,
        "tools":{"shell_enabled":false,"mcp_servers":[old_helper,unrelated.clone()]}}))).await.unwrap();
    let before_config = std::fs::read(&config_path).unwrap();
    let before_parent = std::fs::read(&global_path).unwrap();
    assert!(!state_path.exists(), "the reviewed generation is the actual initial zero");

    // Establish a real durable owner, then drop the opener before the
    // consumer under test restores that same already-governed realm.
    let (_, bundle) = meerkat::storage_provider::open_realm_persistence_with_layout(
        storage.layout.clone(), storage.locator.realm.as_str(), None,
        Some(meerkat_store::RealmOrigin::Explicit),
    ).await.unwrap();
    drop(bundle);
    let principals = StockPrincipals::for_local_operator(&storage.locator.realm).unwrap();
    govern_stock_realm(&storage.locator.state_root, storage.locator.realm.as_str(), &principals).await.unwrap();
    let paths = meerkat_store::realm_paths_in(&storage.locator.state_root, storage.locator.realm.as_str());
    let owner_path = meerkat_store::grant_owner_store_path(&paths.root);
    let before_owner = std::fs::read(&owner_path).unwrap();
    assert!(!before_owner.is_empty(), "the oracle is a real native grant owner");
    assert!(!state_path.exists(), "governance did not publish a config generation");

    let store = Arc::new(ObstructGenerationAfterPatch { file, state_path: state_path.clone(), patched: AtomicUsize::new(0) });
    let runtime = ConfigRuntime::new(store.clone(), state_path.clone());
    let artifacts = directory.path().join("artifacts");
    std::fs::create_dir(&artifacts).unwrap();
    let raw = br#"{"mcpServers":{"toolkit-setup":{"command":"/usr/bin/env","args":["-i","/bin/true","reviewed-helper"]}}}"#;
    let connection = crate::setup::PreparedSetupConnection::from_preparer_output(raw).unwrap();
    let mut expected_helper = crate::setup::PreparedSetupConnection::from_preparer_output(raw).unwrap().into_server();
    expected_helper.access = Some(meerkat_core::McpServerAccess::allowing(
        crate::setup::PREPARATION_TOOLS.iter().chain(crate::setup::ACQUISITION_TOOLS.iter()).map(|name| (*name).to_owned()),
    ));
    let prepared = crate::resources::PreparedResources { connection, artifact_directory: artifacts.clone() };
    let session = Session::new();
    let (approvals, mut presenter) = LocalApprovals::create_paired_host(&artifacts).unwrap();
    approvals.bind_session(session.id()).unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let mut input = TerminalInput::scripted(rx);
    let mut output = ApproveRenderedReview { bytes: Vec::new(), input: tx, decisions: 0 };
    let interrupt = std::future::pending::<io::Result<()>>();
    tokio::pin!(interrupt);
    let result = SessionStartup::prepare_standalone_with_config(storage.clone(), session, prepared, runtime,
        &approvals, &mut presenter, &mut input, &mut output, interrupt.as_mut()).await;
    approvals.retire();
    let error = match result {
        Err(error) => error,
        Ok(startup) => {
            if let Some(mut startup) = startup { startup.abort_standalone(&mut output).await.unwrap(); }
            panic!("a failed native generation publication must return its failure");
        }
    };
    let ConnectError::Native { operation, source } = error else { panic!("the native write failure is primary"); };
    assert_eq!(operation, "Save reviewed setup configuration; outcome may be partial");
    assert!(matches!(source.downcast_ref::<ConfigRuntimeError>(), Some(ConfigRuntimeError::Io(_))),
        "the error must be native generation publication, not delegated store refusal");
    assert_eq!(store.patched.load(Ordering::SeqCst), 1);
    assert!(state_path.is_dir());
    assert_ne!(std::fs::read(&config_path).unwrap(), before_config);
    let after = store.file.get().await.unwrap();
    assert_eq!(after.max_tokens, Some(1234));
    assert!(!after.tools.shell_enabled);
    assert_eq!(after.tools.mcp_servers, vec![expected_helper, unrelated]);
    assert_eq!(std::fs::read(&global_path).unwrap(), before_parent, "parent configuration is untouched");
    assert_eq!(std::fs::read(&owner_path).unwrap(), before_owner, "no grant was appended or owner recreated");
    assert_eq!(meerkat_store::read_realm_posture_in(&storage.locator.state_root, storage.locator.realm.as_str()).await.unwrap(), meerkat_store::RealmPosture::Governed);
    assert_eq!(output.decisions, 1);
    let output = String::from_utf8(output.bytes).unwrap();
    assert!(output.contains("Decision recorded. Waiting for the native outcome."));
    assert!(output.contains("Saving the setup connection failed and may be partial. No setup grant or retry was started."));
    let retained = std::fs::read_to_string(artifacts.join("standalone-setup-configuration.txt")).unwrap();
    assert!(retained.contains("Native configuration write failed and may be partial"));
    assert!(artifacts.join("standalone-realm-governance.txt").is_file());
    assert!(!artifacts.join("standalone-mcp-grant-request.txt").exists(),
        "the mandatory request record immediately before native issuance was not reached");
    assert!(!artifacts.join("standalone-mcp-grant-outcome.txt").exists());
    // Stage entry follows a successfully observed reviewed grant in production.
    // The exact early return and unchanged owner above do not constitute a
    // separately instrumented stage-entry counter or a successful startup.
}


#[tokio::test]
async fn ordinary_continuation_reopens_exact_history_and_current_config_without_reissuing() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        use meerkat::governed_realm::stock::{StockPrincipals, govern_stock_realm};
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions {
            context_root: directory.path().to_owned(),
            state_root: Some(directory.path().join("state")),
            user_config_root: Some(directory.path().join("home")),
        }.resolve("continuation").unwrap();
        let (_, bundle) = meerkat::storage_provider::open_realm_persistence_with_layout(
            storage.layout.clone(), storage.locator.realm.as_str(), None,
            Some(meerkat_store::RealmOrigin::Explicit),
        ).await.unwrap();
        let mut history = Session::new();
        history.push(meerkat_core::Message::User(meerkat_core::UserMessage::text("retained setup history")));
        bundle.session_store().save(&history).await.unwrap();
        drop(bundle);
        let principals = StockPrincipals::for_local_operator(&storage.locator.realm).unwrap();
        govern_stock_realm(&storage.locator.state_root, storage.locator.realm.as_str(), &principals).await.unwrap();
        let paths = meerkat_store::realm_paths_in(&storage.locator.state_root, storage.locator.realm.as_str());
        let owner_path = meerkat_store::grant_owner_store_path(&paths.root);
        let config_path = crate::realm::config_source(&storage).unwrap().config_doc_path(&storage.locator.realm);
        let file = FileConfigStore::new(config_path.clone(), meerkat_models::canonical());
        let mut server = McpServerConfig::stdio(crate::setup::SERVER_NAME, "/bin/true", vec!["original-supplied-descriptor".into()], Default::default());
        server.access = Some(meerkat_core::McpServerAccess::allowing(["setup_guide"]));
        let unrelated = McpServerConfig::stdio("other-service", "/bin/true", vec!["keep".into()], Default::default());
        file.patch(ConfigDelta(serde_json::json!({"max_tokens":1234,"tools":{"mcp_servers":[server.clone(),unrelated.clone()]}}))).await.unwrap();
        let continuation = StandaloneContinuation { storage:storage.clone(), session_id:history.id().clone(), server:server.clone() };
        let owner_before = std::fs::read(&owner_path).unwrap();
        let config_before = std::fs::read(&config_path).unwrap();
        let resumed = continuation.reopen().await.unwrap();
        assert_eq!(resumed.session.id(), history.id());
        assert_eq!(resumed.session.messages(), history.messages());
        assert_eq!(resumed.server, server);
        assert!(resumed.startup.standalone.is_none(), "ordinary reopen must not retain a prepared stage or ticket");
        assert!(resumed.startup.connector_auth.is_none(), "the old facade is not reused");
        assert!(resumed.startup.persistence.as_ref().unwrap().stock_governance().is_some());
        assert_eq!(resumed.startup.config.max_tokens, Some(1234));
        drop(resumed);
        assert_eq!(std::fs::read(&owner_path).unwrap(), owner_before);
        assert_eq!(std::fs::read(&config_path).unwrap(), config_before);

        // A fresh opener reads unrelated operator changes, while the original
        // helper descriptor and session remain exact. This opens no actor/MCP.
        file.patch(ConfigDelta(serde_json::json!({"max_tokens":2345}))).await.unwrap();
        let resumed = continuation.reopen().await.unwrap();
        assert_eq!(resumed.startup.config.max_tokens, Some(2345));
        assert_eq!(resumed.session.messages(), history.messages());
        drop(resumed);
        assert_eq!(std::fs::read(&owner_path).unwrap(), owner_before, "no startup or service grant was reissued");

        for changed in [None, Some(McpServerConfig::stdio(crate::setup::SERVER_NAME, "/bin/true", vec!["different-artifact-path".into()], Default::default())), {
            let mut changed = server.clone(); changed.access = None; Some(changed)
        }] {
            let servers: Vec<_> = changed.into_iter().chain([unrelated.clone()]).collect();
            file.patch(ConfigDelta(serde_json::json!({"tools":{"mcp_servers":servers}}))).await.unwrap();
            let changed_config = std::fs::read(&config_path).unwrap();
            let error = continuation.reopen().await.err().expect("changed descriptor must refuse the real opener");
            assert!(error.to_string().contains("originally reviewed setup connection"));
            assert_eq!(std::fs::read(&config_path).unwrap(), changed_config);
            assert_eq!(std::fs::read(&owner_path).unwrap(), owner_before);
        }
        file.patch(ConfigDelta(serde_json::json!({"tools":{"mcp_servers":[server.clone(),unrelated]}}))).await.unwrap();
        let absent = StandaloneContinuation { storage:storage.clone(), session_id:SessionId::new(), server };
        let error = absent.reopen().await.err().expect("missing history is not a new setup session");
        assert!(error.to_string().contains("committed setup conversation is unavailable"));
        assert_eq!(std::fs::read(&owner_path).unwrap(), owner_before);
        assert!(!config_path.with_file_name("config_state.json").exists(), "read-only reopen never publishes a configuration generation");
    }).await.expect("ordinary reopen custody controls must settle");
}
