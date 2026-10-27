use super::*;
use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore, PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore, TokenStoreError};
use nix::{pty::openpty, sys::termios::{LocalFlags, tcgetattr}, unistd::write};
use std::{sync::{Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::Duration};
use tokio::sync::Semaphore;

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Output {
    fn text(&self) -> String { String::from_utf8(self.0.lock().unwrap().clone()).unwrap() }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> { self.0.lock().unwrap().extend_from_slice(bytes); Ok(bytes.len()) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
async fn shown(output: &Output, text: &str) {
    while !output.text().contains(text) { tokio::task::yield_now().await; }
}
fn storage(root: &std::path::Path, realm: &str) -> ResolvedStorage {
    crate::RealmOptions { context_root: root.into(), state_root: Some(root.join("state")), user_config_root: Some(root.join("home")) }.resolve(realm).unwrap()
}
fn target(storage: &ResolvedStorage) -> ConnectorOAuthTarget {
    use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef};
    ConnectorOAuthTarget {
        slot: CredentialAccountRef { realm: storage.locator.realm.clone(), account: CredentialAccountId::parse("mail").unwrap() },
        issuer: "https://issuer.invalid".into(), client: "mail-client".into(), resource: "https://api.invalid".into(),
        scopes: ["read".into()].into(), strategy_id: "oidc-userinfo-v1".into(),
        account: meerkat_auth_core::connector_oauth::AccountSelection::Discover,
        client_auth_profile: Some("mail".into()),
    }
}
fn definition(target: &ConnectorOAuthTarget) -> ConnectorClientConfig {
    ConnectorClientConfig { issuer: target.issuer.clone(), client: target.client.clone(), client_auth: ConnectorClientAuthMethod::None,
        authorize_params: Vec::new(), resource_indicator: true, protocol: None, registered_redirect_uris: Vec::new() }
}
fn native_source(storage: &ResolvedStorage) -> RealmConnectorClients {
    RealmConnectorClients::from_realm_source(crate::realm::config_source(storage).unwrap(), storage.locator.realm.clone())
}
async fn put(storage: &ResolvedStorage, persistence: &ProviderAuthPersistence, config: ConnectorClientConfig) {
    let path = crate::realm::config_source(storage).unwrap().config_doc_path(&storage.locator.realm);
    meerkat::put_connector_client(Arc::new(meerkat_core::FileConfigStore::new(path, meerkat_models::canonical())), persistence,
        &storage.locator.realm, "mail", config, ConnectorClientSecretUpdate::Keep).await.unwrap();
}
struct HeldStore {
    inner: Arc<FileTokenStore>, entered: Semaphore, release: Semaphore, saves: AtomicUsize,
}
#[async_trait::async_trait]
impl TokenStore for HeldStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> { self.inner.load(key).await }
    async fn save(&self, key: &TokenKey, value: &PersistedTokens) -> Result<(), TokenStoreError> {
        self.saves.fetch_add(1, Ordering::SeqCst); self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget(); self.inner.save(key, value).await
    }
    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> { self.inner.clear(key).await }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> { self.inner.list().await }
    fn backend_name(&self) -> &'static str { "held-native-file" }
}

/// Actual protected terminal approval, hidden input and native file/secret
/// owners. This does not run OAuth or qualify installed-job selection; the
/// check_selected fixture is explicit and no account/grant is manufactured.
#[tokio::test]
async fn guided_missing_profile_uses_same_realm_native_owners_and_joins_entered_cancel() {
    for (stop, advanced) in [("none", false), ("none", true), ("interrupt", false), ("caller_closed", false)] {
        let directory = tempfile::tempdir().unwrap();
        let storage = storage(directory.path(), "work");
        let mut target = target(&storage);
        let observed_target = target.clone();
        target.client_auth_profile = None; // The terminal chooses the profile on this same target.
        let source = native_source(&storage); // Built before configuration, deliberately live.
        let token_store = Arc::new(FileTokenStore::new(directory.path().join("credentials")));
        let held = Arc::new(HeldStore { inner: token_store.clone(), entered: Semaphore::new(0), release: Semaphore::new(0), saves: AtomicUsize::new(0) });
        let persistence = ProviderAuthPersistence::new(held.clone(), Arc::new(FileLockCoordinator::new(directory.path().join("locks"))));
        let factory = meerkat::AgentFactory::new(directory.path().join("factory")).with_provider_auth_persistence(persistence);
        let (approvals, mut presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
        let session = SessionId::new(); approvals.bind_session(&session).unwrap();
        let (mut caller, receive) = oneshot::channel();
        let mut receive = Some(receive);
        let pair = openpty(None, None).unwrap();
        let observer = pair.slave.try_clone().unwrap(); let original = tcgetattr(&observer).unwrap();
        let mut input = TerminalInput::from_owned_fd(pair.slave);
        let mut output = Output::default(); let seen = output.clone();
        let (cancel, receive_cancel) = oneshot::channel();
        let mut cancel = Some(cancel);
        let cancel_seen = Arc::new(AtomicBool::new(false)); let signal_seen = cancel_seen.clone();
        let interrupt = async { receive_cancel.await.map_err(|_| io::Error::other("fixture signal closed"))?; signal_seen.store(true, Ordering::SeqCst); Ok(()) };
        tokio::pin!(interrupt);
        let secret = "synthetic native secret with spaces";
        let human = async {
            shown(&seen, "Client profile [Enter=public default").await; write(&pair.master, b"mail\n").unwrap();
            shown(&seen, "Client authentication [public/secret]").await; write(&pair.master, b"secret\n").unwrap();
            shown(&seen, "Advanced settings? [y/N]").await;
            let answer: &[u8] = if advanced { b"y\n" } else { b"\n" };
            write(&pair.master, answer).unwrap();
            if advanced {
                shown(&seen, "Send the OAuth resource parameter?").await; write(&pair.master, b"no\n").unwrap();
                shown(&seen, "Provider protocol ID").await; write(&pair.master, b"\n").unwrap();
                shown(&seen, "Additional non-secret sign-in parameter").await; write(&pair.master, b"prompt=consent\n").unwrap();
                while seen.text().matches("Additional non-secret sign-in parameter").count() < 2 {
                    tokio::task::yield_now().await;
                }
                write(&pair.master, b"\n").unwrap();
            }
            shown(&seen, "Type 'approve ").await;
            let text = seen.text(); let id = text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
            write(&pair.master, format!("approve {id}\n").as_bytes()).unwrap();
            shown(&seen, "Connector client secret (hidden)").await;
            assert!(!tcgetattr(&observer).unwrap().local_flags.contains(LocalFlags::ECHO));
            write(&pair.master, format!("{secret}\n").as_bytes()).unwrap();
            held.entered.acquire().await.unwrap().forget();
            if stop == "interrupt" {
                cancel.take().unwrap().send(()).unwrap();
                while !cancel_seen.load(Ordering::SeqCst) { tokio::task::yield_now().await; }
            } else if stop == "caller_closed" {
                drop(receive.take());
            }
            if stop != "none" {
                // This output is emitted only after the actual cancellation
                // branch observes the signal/lost waiter and before it joins.
                shown(&seen, "save has already started and may complete").await;
            }
            assert_eq!(held.saves.load(Ordering::SeqCst), 1);
            assert!(token_store.list().await.unwrap().is_empty());
            assert!(selected(&source, &observed_target).await.unwrap().is_none());
            held.release.add_permits(1);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(prepare(&storage, &factory, &source, &mut target, &session, &approvals, &mut presenter,
                &mut input, &mut output, &mut caller, || Ok(()), interrupt.as_mut()), human)
        }).await.expect("native profile save must settle under fixture bound");
        let result = result.unwrap();
        assert_eq!(target.client_auth_profile.as_deref(), Some("mail"));
        match stop {
            "interrupt" => assert!(matches!(result, Preparation::Stopped)),
            "caller_closed" => assert!(matches!(result, Preparation::CallerGone)),
            _ => assert!(matches!(result, Preparation::Current(Some(_)))),
        }
        assert_eq!(held.saves.load(Ordering::SeqCst), 1);
        let reopened = native_source(&storage);
        let profile = selected(&reopened, &target).await.unwrap().unwrap();
        assert_eq!(profile.resource_indicator, !advanced);
        assert_eq!(profile.protocol, None);
        assert_eq!(profile.authorize_params, if advanced {
            vec![("prompt".to_owned(), "consent".to_owned())]
        } else { Vec::new() });
        if !advanced {
            assert!(!seen.text().contains("Send the OAuth resource parameter?"));
            assert!(!seen.text().contains("Provider protocol ID"));
            assert!(!seen.text().contains("Additional non-secret sign-in parameter"));
        }
        assert!(matches!(&profile.client_secret, Some(meerkat::ConnectorClientSecret::RealmStore { realm }) if realm == &storage.locator.realm));
        assert!(meerkat::load_connector_client_secret(token_store.as_ref(), &storage.locator.realm, &profile.binding()).await.unwrap().as_deref() == Some(secret));
        let path = crate::realm::config_source(&storage).unwrap().config_doc_path(&storage.locator.realm);
        let doc = std::fs::read_to_string(path).unwrap(); assert!(!doc.contains(secret));
        assert!(!seen.text().contains(secret)); assert!(seen.text().contains("was saved in realm work"));
        assert_eq!(tcgetattr(&observer).unwrap(), original); input.check_cleanup().unwrap();
    }
}

#[tokio::test]
async fn live_native_selection_inherits_then_refuses_changed_binding_without_restoring_it() {
    let directory = tempfile::tempdir().unwrap();
    let own = storage(directory.path(), "work"); let global = storage(directory.path(), "global");
    let target = target(&own); let source = native_source(&own);
    let persistence = ProviderAuthPersistence::new(Arc::new(FileTokenStore::new(directory.path().join("credentials"))), Arc::new(FileLockCoordinator::new(directory.path().join("locks"))));
    assert!(selected(&source, &target).await.unwrap().is_none());
    put(&global, &persistence, definition(&target)).await;
    let inherited = selected(&source, &target).await.unwrap().unwrap();
    let mut changed = definition(&target); changed.resource_indicator = false;
    put(&own, &persistence, changed.clone()).await;
    assert!(recheck(&source, &target, Some(&inherited)).await.is_err());
    let actual = selected(&source, &target).await.unwrap().unwrap(); assert!(!actual.resource_indicator);
    // A same-name definition for another client is refused without overwrite.
    changed.client = "another-client".into(); put(&own, &persistence, changed).await;
    assert!(selected(&source, &target).await.is_err());
    let public = ConnectorOAuthTarget { client_auth_profile: None, ..target };
    assert!(selected(&source, &public).await.unwrap().is_none());
    assert!(recheck(&source, &public, None).await.is_ok());
    assert_eq!(source.current_profiles().await.unwrap()[0].client, "another-client");
}

#[tokio::test]
async fn pre_entry_cancellation_never_polls_the_native_update() {
    let polled = AtomicUsize::new(0);
    let result = settle(async { polled.fetch_add(1, Ordering::SeqCst); }, async { Ok(Stop::Requested) }, &mut Vec::new()).await.unwrap();
    assert!(matches!(result, Save::Unentered(Stop::Requested))); assert_eq!(polled.load(Ordering::SeqCst), 0);
}

#[test]
fn native_unknown_update_detail_never_becomes_terminal_output_or_no_effect_claim() {
    let error = update_error(ConnectorClientUpdateError::OutcomeUnknown("synthetic-secret".into())).to_string();
    assert!(!error.contains("synthetic-secret")); assert!(error.contains("may have written")); assert!(error.contains("Do not repeat automatically"));
    assert!(!error.contains("did not start"));
    let error = update_error(ConnectorClientUpdateError::Ownership("synthetic-secret".into())).to_string();
    assert!(!error.contains("synthetic-secret")); assert!(error.contains("did not start"));
    let error = update_error(ConnectorClientUpdateError::ConfigFailedAfterSecretStored {
        id: "mail".into(), error: meerkat_core::ConfigError::MissingField("synthetic-secret".into()),
    }).to_string();
    assert!(!error.contains("synthetic-secret")); assert!(error.contains("secret was stored"));
    assert!(error.contains("settlement is unknown")); assert!(!error.contains("No configuration was saved"));
    let error = update_error(ConnectorClientUpdateError::SecretCleanupFailed {
        id: "mail".into(), error: meerkat::ConnectorLoginError::ClientConfig("synthetic-secret".into()),
    }).to_string();
    assert!(!error.contains("synthetic-secret")); assert!(error.contains("configuration was saved"));
    assert!(error.contains("cleanup failed")); assert!(!error.contains("No configuration was saved"));
}

/// The actual terminal approval owner remains open while the native document
/// changes. This tests the account review's live-binding predicate, not OAuth
/// account verification or the native finalizer's separate write custody.
#[tokio::test]
async fn profile_changed_during_account_review_cannot_qualify_the_retained_consent() {
    let directory = tempfile::tempdir().unwrap(); let storage = storage(directory.path(), "work");
    let target = target(&storage); let source = native_source(&storage);
    let persistence = ProviderAuthPersistence::new(Arc::new(FileTokenStore::new(directory.path().join("credentials"))), Arc::new(FileLockCoordinator::new(directory.path().join("locks"))));
    put(&storage, &persistence, definition(&target)).await;
    let snapshot = selected(&source, &target).await.unwrap().unwrap();
    let (approvals, mut presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    let session = SessionId::new(); approvals.bind_session(&session).unwrap();
    let (mut caller, _receive) = oneshot::channel();
    let (send, receive) = tokio::sync::mpsc::channel(1);
    let mut input = TerminalInput::scripted(receive);
    let mut output = Output::default(); let seen = output.clone();
    let request = review_request(&session, "Review installed account access", json!({"phase":"account_access", "realm":"work", "client_profile":display(Some(&snapshot))})).unwrap();
    let reviewing = async {
        let interrupt = std::future::pending::<std::io::Result<()>>();
        tokio::pin!(interrupt);
        let ReviewOutcome::Decided(record) = review(&approvals, &mut presenter, &mut input, &mut output, request, &mut caller, interrupt.as_mut()).await.unwrap() else { panic!("actual decision required") };
        assert_eq!(record.status, ApprovalStatus::Approved);
        recheck(&source, &target, Some(&snapshot)).await
    };
    let human = async {
        shown(&seen, "Type 'approve ").await;
        let text = seen.text(); let id = text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
        let mut changed = definition(&target); changed.resource_indicator = false;
        put(&storage, &persistence, changed).await;
        send.send(Ok(format!("approve {id}"))).await.unwrap();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(reviewing, human) }).await.unwrap();
    assert!(result.unwrap_err().to_string().contains("changed or disappeared after review"));
    assert!(!selected(&source, &target).await.unwrap().unwrap().resource_indicator);
}

mod turn;

#[tokio::test]
async fn missing_secret_repair_preserves_exact_defining_realm_and_denial_has_no_write() {
    for (inherited, approve) in [(false,false),(false,true),(true,false),(true,true)] {
        let directory=tempfile::tempdir().unwrap();
        let selected_storage=storage(directory.path(),"work");
        let defining_storage=storage(directory.path(),if inherited {"global"} else {"work"});
        let mut target=target(&selected_storage); let source=native_source(&selected_storage);
        let tokens=Arc::new(FileTokenStore::new(directory.path().join("credentials")));
        let persistence=ProviderAuthPersistence::new(tokens.clone(),Arc::new(FileLockCoordinator::new(directory.path().join("locks"))));
        let mut client=definition(&target); client.client_auth=ConnectorClientAuthMethod::ClientSecretPost;
        let path=crate::realm::config_source(&defining_storage).unwrap().config_doc_path(&defining_storage.locator.realm);
        meerkat::put_connector_client(Arc::new(meerkat_core::FileConfigStore::new(path,meerkat_models::canonical())),
            &persistence,&defining_storage.locator.realm,"mail",client,ConnectorClientSecretUpdate::Set("expired fixture secret".into())).await.unwrap();
        let before=selected(&source,&target).await.unwrap().unwrap();
        let key=TokenKey::connector_client_secret(defining_storage.locator.realm.clone(),&before.binding()).unwrap();
        tokens.clear(&key).await.unwrap();
        assert!(meerkat::load_connector_client_secret(tokens.as_ref(),&defining_storage.locator.realm,&before.binding()).await.unwrap().is_none());
        let selected_path=crate::realm::config_source(&selected_storage).unwrap().config_doc_path(&selected_storage.locator.realm);
        let own_before=std::fs::read(&selected_path).ok();
        let factory=meerkat::AgentFactory::new(directory.path().join("factory")).with_provider_auth_persistence(persistence);
        let (approvals,mut presenter)=LocalApprovals::create_paired_host(directory.path()).unwrap();
        let session=SessionId::new(); approvals.bind_session(&session).unwrap();
        let (mut caller,_receive)=oneshot::channel();
        let pair=openpty(None,None).unwrap(); let mut input=TerminalInput::from_owned_fd(pair.slave);
        let mut output=Output::default(); let seen=output.clone();
        let interrupt=std::future::pending::<io::Result<()>>(); tokio::pin!(interrupt);
        let secret="protected replacement fixture secret";
        let human=async {
            shown(&seen,"Type 'approve ").await;
            assert!(seen.text().contains(&format!("realm {} (selected setup realm work)",defining_storage.locator.realm.as_str())));
            assert!(!seen.text().contains("Client authentication [public/secret]"));
            let text=seen.text(); let id=text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
            write(&pair.master,format!("{} {id}\n",if approve {"approve"} else {"deny"}).as_bytes()).unwrap();
            if approve {
                shown(&seen,"Connector client secret (hidden)").await;
                write(&pair.master,format!("{secret}\n").as_bytes()).unwrap();
            }
        };
        let (result,())=tokio::time::timeout(Duration::from_secs(10),async {
            tokio::join!(prepare(&selected_storage,&factory,&source,&mut target,&session,&approvals,&mut presenter,
                &mut input,&mut output,&mut caller,||Ok(()),interrupt.as_mut()),human)
        }).await.expect("bounded existing-secret repair");
        if approve { assert!(matches!(result.unwrap(),Preparation::Current(Some(_)))); }
        else { assert!(matches!(result.unwrap(),Preparation::Denied)); }
        let after=selected(&source,&target).await.unwrap().unwrap();
        assert_eq!(before.binding(),after.binding());
        assert!(after.client_secret==before.client_secret);
        let stored=meerkat::load_connector_client_secret(tokens.as_ref(),&defining_storage.locator.realm,&after.binding()).await.unwrap();
        assert_eq!(stored.as_deref(),approve.then_some(secret));
        if inherited { assert_eq!(std::fs::read(&selected_path).ok(),own_before,"never shadow an inherited definition"); }
        assert!(!seen.text().contains(secret));
        input.check_cleanup().unwrap();
    }
}
