use super::*;
use meerkat_auth_core::auth_store::TokenStore;
use meerkat_core::{ConfigError, ConfigRuntimeError, ConfigStore, ConfigWarning, FileConfigStore};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Default)]
struct Output(Arc<std::sync::Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
async fn shown(output: &Output, text: &str) {
    while !String::from_utf8_lossy(&output.0.lock().unwrap()).contains(text) {
        tokio::task::yield_now().await;
    }
}

fn storage(root: &std::path::Path) -> meerkat_core::ResolvedStorage {
    crate::RealmOptions {
        context_root: root.to_owned(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    }
    .resolve("work")
    .unwrap()
}

fn request() -> CreateSessionModelResolutionRequest {
    CreateSessionModelResolutionRequest {
        model: Some("gpt-6-astra".into()),
        ..Default::default()
    }
}

fn name() -> BindingId {
    BindingId::parse("first_key").unwrap()
}

#[tokio::test]
async fn first_target_preserves_parent_presence_and_unrelated_fields() {
    let root = tempfile::tempdir().unwrap();
    let storage = storage(root.path());
    let source = crate::realm::config_source(&storage).unwrap();
    let global_path = storage.layout.user_rkat_root().unwrap().join("config.toml");
    let path = source.config_doc_path(&storage.locator.realm);
    std::fs::create_dir_all(global_path.parent().unwrap()).unwrap();
    std::fs::write(&global_path, concat!(
        "max_tokens = 6789\n[tools]\nshell_enabled = false\n[agent]\nmodel = \"gpt-6-astra\"\n",
        "[realm.global.backend.first_key]\nprovider = \"anthropic\"\nbackend_kind = \"anthropic_api\"\n",
        "[realm.global.auth.first_key]\nprovider = \"anthropic\"\nauth_method = \"api_key\"\nsource = {kind = \"managed_store\"}\n",
    )).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "max_tokens = 2345\n[realm.work]\nparent = \"global\"\n",
    )
    .unwrap();
    let parent = std::fs::read(&global_path).unwrap();
    let mut startup = SessionStartup::open(storage.clone()).await.unwrap();
    let runtime = crate::realm::own_config_runtime(&storage).unwrap();
    let prepared = prepare(
        &startup,
        &runtime,
        request(),
        name(),
        "gpt-6-astra",
        Provider::OpenAI,
    )
    .await
    .unwrap();
    assert_eq!(
        prepared.review.target.auth_binding.realm,
        storage.locator.realm
    );
    assert_eq!(
        prepared.review.request.model.as_deref(),
        Some("gpt-6-astra")
    );
    assert!(
        prepared.previous_default_model.is_none(),
        "matching ordinary model must not be pinned in the head"
    );
    let mut output = Vec::new();
    let interrupt = std::future::pending::<io::Result<()>>();
    tokio::pin!(interrupt);
    let review = save(
        &mut startup,
        &runtime,
        prepared,
        &mut output,
        interrupt.as_mut(),
    )
    .await
    .unwrap()
    .unwrap();
    let own = own_document(&startup).await.unwrap().unwrap();
    assert_eq!(own["max_tokens"], 2345);
    assert!(
        own.get("agent").is_none(),
        "an already matching ordinary model must not be pinned in the own document"
    );
    assert!(
        own.get("tools").is_none(),
        "inherited tools must not be copied into the head"
    );
    assert_eq!(own["realm"]["work"]["parent"], "global");
    assert_eq!(own["realm"]["work"]["default_binding"], "first_key");
    assert_eq!(
        own["realm"]["work"]["backend"]["first_key"]["provider"],
        "openai"
    );
    assert!(
        own["realm"].get("global").is_none(),
        "same-named inherited profiles are not copied into the head"
    );
    assert_eq!(std::fs::read(&global_path).unwrap(), parent);
    review
        .verify(
            &crate::realm::read_config(&storage).await.unwrap(),
            &storage.locator.realm,
        )
        .unwrap();
    assert!(
        startup.persistence.is_none(),
        "config setup alone must not open another runtime"
    );
    assert!(startup.standalone.is_none());
}

#[tokio::test]
async fn first_target_refuses_profile_collision_and_present_default() {
    for body in [
        "[realm.work.backend.first_key]\nprovider = \"openai\"\nbackend_kind = \"openai_api\"\n",
        "[realm.work.auth.first_key]\nprovider = \"openai\"\nauth_method = \"api_key\"\nsource = {kind = \"managed_store\"}\n",
        "[realm.work]\ndefault_binding = \"prior\"\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        let source = crate::realm::config_source(&storage).unwrap();
        let path = source.config_doc_path(&storage.locator.realm);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        let startup = SessionStartup::open(storage.clone()).await.unwrap();
        let runtime = crate::realm::own_config_runtime(&storage).unwrap();
        assert!(
            prepare(
                &startup,
                &runtime,
                request(),
                name(),
                "gpt-6-astra",
                Provider::OpenAI
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        assert!(!path.with_file_name("config_state.json").exists());
    }
    // An ancestor's different realm may reuse a profile name (covered above).
    // Its same-id realm section is replaced by whole-section native merge, so
    // preserved bytes alone would not prove preserved effective settings.
    // The user document composes into a parentless realm only once some
    // document configures [realm.global]; without it the section is inert.
    for body in [
        "[realm.work]\nparent = \"global\"\n",
        "[realm.global]\n[realm.work.backend.unrelated]\nprovider = \"openai\"\nbackend_kind = \"openai_api\"\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        let global_path = storage.layout.user_rkat_root().unwrap().join("config.toml");
        let path = crate::realm::config_source(&storage)
            .unwrap()
            .config_doc_path(&storage.locator.realm);
        std::fs::create_dir_all(global_path.parent().unwrap()).unwrap();
        std::fs::write(&global_path, body).unwrap();
        let startup = SessionStartup::open(storage.clone()).await.unwrap();
        let runtime = crate::realm::own_config_runtime(&storage).unwrap();
        let error = match prepare(
            &startup,
            &runtime,
            request(),
            name(),
            "gpt-6-astra",
            Provider::OpenAI,
        )
        .await
        {
            Err(error) => error,
            Ok(_) => {
                panic!("first-target setup must not hide unrelated effective same-realm settings")
            }
        };
        assert!(
            error
                .to_string()
                .contains("Add the connection manually to the native realm configuration")
        );
        assert_eq!(std::fs::read_to_string(&global_path).unwrap(), body);
        assert!(!path.exists());
        assert!(!path.with_file_name("config_state.json").exists());
    }
}

#[tokio::test]
async fn first_target_original_native_generation_refuses_a_concurrent_writer() {
    let root = tempfile::tempdir().unwrap();
    let storage = storage(root.path());
    let runtime = crate::realm::own_config_runtime(&storage).unwrap();
    // Establish the native serializer's stable empty document first, so the
    // second equal-value write changes the real generation but not raw values.
    runtime
        .patch(
            ConfigDelta(serde_json::json!({"max_tokens": 2345})),
            Some(0),
        )
        .await
        .unwrap();
    let mut startup = SessionStartup::open(storage.clone()).await.unwrap();
    let prepared = prepare(
        &startup,
        &runtime,
        request(),
        name(),
        "gpt-6-astra",
        Provider::OpenAI,
    )
    .await
    .unwrap();
    let expected = prepared.generation;
    runtime
        .patch(
            ConfigDelta(serde_json::json!({"max_tokens": 2345})),
            Some(expected),
        )
        .await
        .unwrap();
    assert_eq!(own_document(&startup).await.unwrap(), prepared.own_document);
    let mut output = Vec::new();
    let interrupt = std::future::pending::<io::Result<()>>();
    tokio::pin!(interrupt);
    let result = save(
        &mut startup,
        &runtime,
        prepared,
        &mut output,
        interrupt.as_mut(),
    )
    .await;
    let Err(ConnectError::Native { source, .. }) = result else {
        panic!("native CAS must refuse");
    };
    assert!(matches!(source.downcast_ref::<ConfigRuntimeError>(),
        Some(ConfigRuntimeError::GenerationConflict { expected: wanted, current }) if *wanted == expected && *current == expected + 1));
    let own = own_document(&startup).await.unwrap().unwrap();
    assert!(
        own.get("realm")
            .and_then(|v| v.get("work"))
            .and_then(|v| v.get("binding"))
            .is_none()
    );
    let output = String::from_utf8(output).unwrap();
    assert!(!output.contains("Enter the API key"));
    assert!(output.contains("This operation wrote no configuration"));
    assert!(!output.contains("may be partial"));
}

struct ObstructGenerationAfterPatch {
    file: FileConfigStore,
    state_path: std::path::PathBuf,
    patched: AtomicUsize,
}

#[async_trait::async_trait]
impl ConfigStore for ObstructGenerationAfterPatch {
    async fn get(&self) -> Result<Config, ConfigError> {
        self.file.get().await
    }
    async fn set(&self, config: Config) -> Result<(), ConfigError> {
        self.file.set(config).await
    }
    async fn patch_preview(
        &self,
        delta: &ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        self.file.patch_preview(delta).await
    }
    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let config = self.file.patch(delta).await?;
        self.patched.fetch_add(1, Ordering::SeqCst);
        tokio::fs::create_dir(&self.state_path).await?;
        Ok(config)
    }
}

#[tokio::test]
async fn first_target_partial_publication_stops_without_credential_or_retry() {
    let root = tempfile::tempdir().unwrap();
    let storage = storage(root.path());
    let path = crate::realm::config_source(&storage)
        .unwrap()
        .config_doc_path(&storage.locator.realm);
    let state_path = path.with_file_name("config_state.json");
    let store = Arc::new(ObstructGenerationAfterPatch {
        file: FileConfigStore::new(path.clone(), meerkat_models::canonical()),
        state_path: state_path.clone(),
        patched: AtomicUsize::new(0),
    });
    let runtime = ConfigRuntime::new(store.clone(), state_path.clone());
    let mut startup = SessionStartup::open(storage.clone()).await.unwrap();
    let prepared = prepare(
        &startup,
        &runtime,
        request(),
        name(),
        "gpt-6-astra",
        Provider::OpenAI,
    )
    .await
    .unwrap();
    assert!(prepared.own_document.is_none());
    let mut output = Vec::new();
    let interrupt = std::future::pending::<io::Result<()>>();
    tokio::pin!(interrupt);
    let result = save(
        &mut startup,
        &runtime,
        prepared,
        &mut output,
        interrupt.as_mut(),
    )
    .await;
    let Err(ConnectError::Native { operation, source }) = result else {
        panic!("partial native failure must be retained");
    };
    assert_eq!(
        operation,
        "Save reviewed model target; outcome may be partial"
    );
    assert!(matches!(
        source.downcast_ref::<ConfigRuntimeError>(),
        Some(ConfigRuntimeError::Io(_))
    ));
    assert_eq!(store.patched.load(Ordering::SeqCst), 1);
    assert!(state_path.is_dir());
    let own = own_document(&startup).await.unwrap().unwrap();
    assert_eq!(own["realm"]["work"]["default_binding"], "first_key");
    assert!(startup.persistence.is_none());
    assert!(startup.standalone.is_none());
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("Reviewed target currently confirmed: yes"));
    assert!(!output.contains("Enter the API key"));
}

struct HeldPatch {
    file: FileConfigStore,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    patched: AtomicUsize,
}

#[async_trait::async_trait]
impl ConfigStore for HeldPatch {
    async fn get(&self) -> Result<Config, ConfigError> {
        self.file.get().await
    }
    async fn set(&self, config: Config) -> Result<(), ConfigError> {
        self.file.set(config).await
    }
    async fn patch_preview(
        &self,
        delta: &ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        self.file.patch_preview(delta).await
    }
    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        self.patched.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        self.file.patch(delta).await
    }
}

#[tokio::test]
async fn first_target_cancellation_joins_entered_native_write_without_continuation() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        let path = crate::realm::config_source(&storage).unwrap().config_doc_path(&storage.locator.realm);
        let store = Arc::new(HeldPatch { file: FileConfigStore::new(path.clone(), meerkat_models::canonical()),
            entered: tokio::sync::Semaphore::new(0), release: tokio::sync::Semaphore::new(0), patched: AtomicUsize::new(0) });
        let runtime = ConfigRuntime::new(store.clone(), path.with_file_name("config_state.json"));
        let mut startup = SessionStartup::open(storage.clone()).await.unwrap();
        let prepared = prepare(&startup, &runtime, request(), name(), "gpt-6-astra", Provider::OpenAI).await.unwrap();
        let (send, receive) = tokio::sync::oneshot::channel();
        let interrupt = async { receive.await.map_err(|_| io::Error::other("closed test signal")) };
        tokio::pin!(interrupt);
        let mut output = Output::default();
        let observed = output.clone();
        let saving = save(&mut startup, &runtime, prepared, &mut output, interrupt.as_mut());
        tokio::pin!(saving);
        tokio::select! { biased;
            result = saving.as_mut() => panic!("write finished before held native store: {}", result.is_ok()),
            permit = store.entered.acquire() => permit.unwrap().forget(),
        }
        send.send(()).unwrap();
        tokio::select! { biased;
            result = saving.as_mut() => panic!("cancellation dropped the native write: {}", result.is_ok()),
            () = shown(&observed, "Waiting for native model configuration to settle") => {},
        }
        assert!(!path.exists());
        store.release.add_permits(1);
        assert!(saving.await.unwrap().is_none());
        assert_eq!(store.patched.load(Ordering::SeqCst), 1);
        assert!(path.is_file());
    }).await.expect("entered native publication must settle within the control bound");
}

struct ApproveHelper {
    bytes: Vec<u8>,
    input: tokio::sync::mpsc::Sender<io::Result<String>>,
    sent: bool,
}
impl Write for ApproveHelper {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if !self.sent
            && let Some((_, suffix)) = std::str::from_utf8(&self.bytes)
                .unwrap()
                .split_once("Type 'approve ")
        {
            let (id, _) = suffix.split_once('\'').unwrap();
            self.input.try_send(Ok(format!("approve {id}"))).unwrap();
            self.sent = true;
        }
        Ok(())
    }
}

#[tokio::test]
async fn first_target_refresh_preserves_actual_staged_auth_owner() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        let artifacts = root.path().join("artifacts");
        std::fs::create_dir(&artifacts).unwrap();
        let connection = crate::setup::PreparedSetupConnection::from_preparer_output(
            br#"{"mcpServers":{"toolkit-setup":{"command":"/bin/true","args":["first-target-stage"]}}}"#,
        ).unwrap();
        let resources = crate::resources::PreparedResources { connection, artifact_directory: artifacts.clone() };
        let session = meerkat::Session::new();
        let session_id = session.id().clone();
        let (approvals, mut presenter) = crate::approval::LocalApprovals::create_paired_host(&artifacts).unwrap();
        approvals.bind_session(session.id()).unwrap();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let mut input = TerminalInput::scripted(receive);
        let mut output = ApproveHelper { bytes: Vec::new(), input: send, sent: false };
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        let mut startup = SessionStartup::prepare_standalone(storage.clone(), session, resources,
            &approvals, &mut presenter, &mut input, &mut output, interrupt.as_mut()).await.unwrap().unwrap();
        approvals.retire();
        // No observation assertion can unwind past a live stage. A fallible
        // observation releases its local auth handles before the joined abort.
        let observations = async {
            let before = startup.provider_auth_runtime_authority().await?;
            let owner_path = meerkat_store::grant_owner_store_path(
                &meerkat_store::realm_paths_in(&storage.locator.state_root, "work").root);
            let before_owner = std::fs::read(&owner_path)
                .map_err(|error| ConnectError::native("Read prior control grant owner", error))?;
            let runtime = crate::realm::own_config_runtime(&storage)?;
            let prepared = prepare(&startup, &runtime, request(), name(), "gpt-6-astra", Provider::OpenAI).await?;
            let review = save(&mut startup, &runtime, prepared, &mut output, interrupt.as_mut()).await?
                .ok_or_else(|| invalid("The stage control unexpectedly cancelled before observation"))?;
            let after = startup.provider_auth_runtime_authority().await?;
            let same_lease = before.generated_auth_lease_handle().same_owner(&after.generated_auth_lease_handle());
            let same_oauth = std::ptr::addr_eq(Arc::as_ptr(&before.oauth_flow_authority()), Arc::as_ptr(&after.oauth_flow_authority()));
            let retained_session = startup.standalone.as_ref().map(|staged| staged.session.id().clone());
            let no_second_bundle = startup.persistence.is_none();
            let after_owner = std::fs::read(&owner_path)
                .map_err(|error| ConnectError::native("Read later control grant owner", error))?;
            let current_target = review.verify(&startup.config, &storage.locator.realm);
            drop(before);
            drop(after);
            Ok::<_, ConnectError>((same_lease, same_oauth, retained_session, no_second_bundle,
                before_owner == after_owner, current_target))
        }.await;
        let cleanup = startup.abort_standalone(&mut output).await;
        cleanup.unwrap();
        let (same_lease, same_oauth, retained_session, no_second_bundle, unchanged_owner, current_target) = observations.unwrap();
        assert!(same_lease);
        assert!(same_oauth);
        assert_eq!(retained_session, Some(session_id));
        assert!(no_second_bundle, "the standalone stage did not gain a second realm bundle");
        assert!(unchanged_owner);
        current_target.unwrap();
    }).await.expect("actual first-target stage cleanup must settle");
}

struct HeldTokens {
    file: Arc<meerkat_auth_core::auth_store::FileTokenStore>,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    saves: AtomicUsize,
}

#[async_trait::async_trait]
impl meerkat_auth_core::auth_store::TokenStore for HeldTokens {
    async fn load(
        &self,
        key: &meerkat_auth_core::auth_store::TokenKey,
    ) -> Result<
        Option<meerkat_auth_core::auth_store::PersistedTokens>,
        meerkat_auth_core::auth_store::TokenStoreError,
    > {
        self.file.load(key).await
    }
    async fn save(
        &self,
        key: &meerkat_auth_core::auth_store::TokenKey,
        tokens: &meerkat_auth_core::auth_store::PersistedTokens,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        self.file.save(key, tokens).await
    }
    async fn clear(
        &self,
        key: &meerkat_auth_core::auth_store::TokenKey,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        self.file.clear(key).await
    }
    async fn list(
        &self,
    ) -> Result<
        Vec<meerkat_auth_core::auth_store::TokenKey>,
        meerkat_auth_core::auth_store::TokenStoreError,
    > {
        self.file.list().await
    }
    fn backend_name(&self) -> &'static str {
        "held-first-target-native-file"
    }
}

#[tokio::test]
async fn first_target_change_during_native_key_save_keeps_saved_effect_and_refuses_continuation() {
    use meerkat_auth_core::auth_store::{
        FileLockCoordinator, FileTokenStore, ProviderAuthPersistence, TokenKey,
    };
    tokio::time::timeout(Duration::from_secs(15), async {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        let path = crate::realm::config_source(&storage)
            .unwrap()
            .config_doc_path(&storage.locator.realm);
        let runtime = crate::realm::own_config_runtime(&storage).unwrap();
        let mut startup = SessionStartup::open(storage.clone()).await.unwrap();
        let prepared = prepare(
            &startup,
            &runtime,
            request(),
            name(),
            "gpt-6-astra",
            Provider::OpenAI,
        )
        .await
        .unwrap();
        let mut output = Output::default();
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        let review = save(
            &mut startup,
            &runtime,
            prepared,
            &mut output,
            interrupt.as_mut(),
        )
        .await
        .unwrap()
        .unwrap();
        let native_file = Arc::new(FileTokenStore::new(root.path().join("credentials")));
        let held = Arc::new(HeldTokens {
            file: native_file.clone(),
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
            saves: AtomicUsize::new(0),
        });
        let persistence = ProviderAuthPersistence::new(
            held.clone(),
            Arc::new(FileLockCoordinator::new(root.path().join("locks"))),
        );
        startup.factory = startup
            .factory
            .clone()
            .with_provider_auth_persistence(persistence);
        let pair = nix::pty::openpty(None, None).unwrap();
        let observer = pair.slave.try_clone().unwrap();
        let initial = nix::sys::termios::tcgetattr(&observer).unwrap();
        let mut input = TerminalInput::from_owned_fd(pair.slave);
        let observed = output.clone();
        let actor = async {
            shown(&observed, "Enter the API key (hidden)").await;
            assert!(
                !nix::sys::termios::tcgetattr(&observer)
                    .unwrap()
                    .local_flags
                    .contains(nix::sys::termios::LocalFlags::ECHO)
            );
            nix::unistd::write(&pair.master, b"synthetic-first-target-key\n").unwrap();
            held.entered.acquire().await.unwrap().forget();
            FileConfigStore::new(path.clone(), meerkat_models::canonical())
                .patch(ConfigDelta(
                    serde_json::json!({"realm":{"work":{"auth":{"first_key":{
                        "source":{"kind":"env","env":"FIRST_TARGET_ABSENT_KEY"}
                    }}}}}),
                ))
                .await
                .unwrap();
            held.release.add_permits(1);
        };
        let (result, ()) = tokio::join!(
            api_key_login_reviewed(
                &mut startup,
                &review.target,
                Some(&review),
                &mut input,
                &mut output,
                interrupt.as_mut()
            ),
            actor
        );
        assert!(
            result.is_err(),
            "a completed save cannot open a changed target from an old Config"
        );
        assert_eq!(held.saves.load(Ordering::SeqCst), 1);
        let key = TokenKey::from_credential_identity(&review.target.credential_identity);
        assert_eq!(
            native_file
                .load(&key)
                .await
                .unwrap()
                .unwrap()
                .primary_secret
                .as_deref(),
            Some("synthetic-first-target-key")
        );
        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("API key stored by the native credential owner."));
        assert!(!text.contains("synthetic-first-target-key"));
        assert_eq!(nix::sys::termios::tcgetattr(&observer).unwrap(), initial);
        input.check_cleanup().unwrap();
    })
    .await
    .expect("real pending native save must settle before currentness refusal");
}

#[tokio::test]
async fn first_target_refuses_changed_native_default_before_creation() {
    let root = tempfile::tempdir().unwrap();
    let storage = storage(root.path());
    let runtime = crate::realm::own_config_runtime(&storage).unwrap();
    runtime
        .patch(
            ConfigDelta(serde_json::json!({"agent":{"model":"gpt-6-astra"}})),
            Some(0),
        )
        .await
        .unwrap();
    let startup = SessionStartup::open(storage.clone()).await.unwrap();
    runtime
        .patch(
            ConfigDelta(serde_json::json!({"agent":{"model":"claude-opus-5-5"}})),
            Some(1),
        )
        .await
        .unwrap();
    let before = own_document(&startup).await.unwrap();
    assert!(
        prepare(
            &startup,
            &runtime,
            Default::default(),
            name(),
            "gpt-6-astra",
            Provider::OpenAI
        )
        .await
        .is_err()
    );
    assert_eq!(own_document(&startup).await.unwrap(), before);
    assert_eq!(runtime.get().await.unwrap().generation, 2);
}
