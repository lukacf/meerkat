use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use meerkat_core::auth::{
    McpServiceSecretPresence, McpServiceSecretRecheck, McpServiceSecretRef,
    McpServiceSecretRefusal, McpServiceSecretWitness, TokenStore, clear_mcp_service_secret,
    save_mcp_service_secret,
};
use meerkat_core::connection::{RealmConfigSection, RealmId};
use meerkat_core::mcp_config::McpTransportConfig;
use meerkat_core::{Config, McpServerConfig};

use super::*;

const SLOT: &str = "SEARCH_API_KEY";

fn realm(id: &str) -> RealmId {
    RealmId::parse(id).expect("realm id")
}

fn search(args: &[&str]) -> McpServerConfig {
    let mut server = McpServerConfig::stdio(
        "search",
        "search-server",
        args.iter().map(|arg| (*arg).to_owned()).collect(),
        HashMap::new(),
    );
    if let McpTransportConfig::Stdio(stdio) = &mut server.transport {
        stdio.secret_env.insert(SLOT.to_owned());
    }
    server
}

fn reference(owner: &str, server: &McpServerConfig) -> McpServiceSecretRef {
    McpServiceSecretRef::for_definition(realm(owner), server, SLOT).expect("declared slot")
}

/// Realm documents by realm id: each realm's own document only.
struct Documents(std::sync::Mutex<BTreeMap<String, Config>>);

impl Documents {
    fn new(docs: impl IntoIterator<Item = (&'static str, Config)>) -> Arc<Self> {
        Arc::new(Self(std::sync::Mutex::new(
            docs.into_iter()
                .map(|(id, doc)| (id.to_owned(), doc))
                .collect(),
        )))
    }

    fn replace(&self, id: &str, doc: Config) {
        self.0.lock().unwrap().insert(id.to_owned(), doc);
    }
}

#[async_trait::async_trait]
impl meerkat_core::RealmConfigSource for Documents {
    async fn config_for_realm(
        &self,
        realm: &RealmId,
    ) -> Result<Option<Config>, meerkat_core::ConfigError> {
        Ok(self.0.lock().unwrap().get(realm.as_str()).cloned())
    }
}

/// A realm document listing `servers`, with `parent` as its parent.
fn document(id: &str, parent: Option<RealmId>, servers: Vec<McpServerConfig>) -> Config {
    let mut doc = Config::default();
    doc.realm.insert(
        id.to_owned(),
        RealmConfigSection {
            parent,
            ..Default::default()
        },
    );
    doc.tools.mcp_servers = servers;
    doc
}

fn store() -> Arc<dyn TokenStore> {
    Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new())
}

/// Store `value` for `reference` as a write does, and return its witness.
async fn written(
    store: &Arc<dyn TokenStore>,
    reference: &McpServiceSecretRef,
    value: &str,
) -> McpServiceSecretWitness {
    let row = save_mcp_service_secret(
        &meerkat_core::AdmissionGuard::none(),
        store.as_ref(),
        reference,
        value,
    )
    .await
    .unwrap();
    reference.witness_of(&row).expect("a named write")
}

#[tokio::test]
async fn a_launch_reads_only_the_defining_realms_exact_row() {
    let defined = search(&["--stdio"]);
    let docs = Documents::new([
        ("global", document("global", None, vec![defined.clone()])),
        (
            "child",
            document("child", Some(RealmId::global()), Vec::new()),
        ),
        ("other", document("other", None, vec![defined.clone()])),
    ]);
    let store = store();
    save_mcp_service_secret(
        &meerkat_core::AdmissionGuard::none(),
        store.as_ref(),
        &reference("global", &defined),
        "global-value",
    )
    .await
    .unwrap();
    save_mcp_service_secret(
        &meerkat_core::AdmissionGuard::none(),
        store.as_ref(),
        &reference("other", &defined),
        "other-value",
    )
    .await
    .unwrap();
    let custody = RealmMcpServiceSecrets::from_realm_source(
        Arc::clone(&docs) as Arc<dyn meerkat_core::RealmConfigSource>,
        realm("child"),
        Arc::clone(&store),
    );

    // An inherited definition reads its defining realm's row (an
    // independently valid inheritance; the unrelated realm's row is never
    // read).
    let values = custody.resolve(&defined).await.expect("inherited value");
    assert_eq!(
        values.into_env().collect::<Vec<_>>(),
        vec![(SLOT.to_owned(), "global-value".to_owned())]
    );

    // The launched definition must be the defining realm's current one.
    assert_eq!(
        custody.resolve(&search(&["--other"])).await,
        Err(McpServiceSecretRefusal::DefinitionChanged)
    );

    // A child repeating the definition defines it itself: its own row only,
    // never the parent's.
    docs.replace(
        "child",
        document("child", Some(RealmId::global()), vec![defined.clone()]),
    );
    assert_eq!(
        custody.resolve(&defined).await,
        Err(McpServiceSecretRefusal::NotStored {
            env: SLOT.to_owned()
        })
    );
    save_mcp_service_secret(
        &meerkat_core::AdmissionGuard::none(),
        store.as_ref(),
        &reference("child", &defined),
        "child-value",
    )
    .await
    .unwrap();
    let launched = custody.resolve(&defined).await.expect("child value");

    // Rotation and removal apply to the next read; a launch that already
    // read keeps its snapshot.
    save_mcp_service_secret(
        &meerkat_core::AdmissionGuard::none(),
        store.as_ref(),
        &reference("child", &defined),
        "rotated",
    )
    .await
    .unwrap();
    assert_eq!(
        launched.into_env().collect::<Vec<_>>(),
        vec![(SLOT.to_owned(), "child-value".to_owned())]
    );
    assert_eq!(
        custody
            .resolve(&defined)
            .await
            .expect("rotated")
            .into_env()
            .collect::<Vec<_>>(),
        vec![(SLOT.to_owned(), "rotated".to_owned())]
    );
    clear_mcp_service_secret(
        &meerkat_core::AdmissionGuard::none(),
        store.as_ref(),
        &reference("child", &defined),
    )
    .await
    .unwrap();
    assert_eq!(
        custody.resolve(&defined).await,
        Err(McpServiceSecretRefusal::NotStored {
            env: SLOT.to_owned()
        })
    );

    // A deleted definition has no owner.
    docs.replace(
        "child",
        document("child", Some(RealmId::global()), Vec::new()),
    );
    docs.replace("global", document("global", None, Vec::new()));
    assert_eq!(
        custody.resolve(&defined).await,
        Err(McpServiceSecretRefusal::NoOwner)
    );

    // A server declaring no secrets reads nothing.
    let plain = McpServerConfig::stdio("plain", "plain", Vec::new(), HashMap::new());
    assert!(custody.resolve(&plain).await.expect("nothing").is_empty());
}

#[tokio::test]
async fn an_observation_names_the_owner_and_presence_without_the_value() {
    let defined = search(&[]);
    let docs = Documents::new([("team", document("team", None, vec![defined.clone()]))]);
    let store = store();
    let custody = RealmMcpServiceSecrets::from_realm_source(
        docs as Arc<dyn meerkat_core::RealmConfigSource>,
        realm("team"),
        Arc::clone(&store),
    );
    let before = custody.observe(&defined).await.expect("observed");
    assert_eq!(before.owner, realm("team"));
    assert_eq!(before.slots[SLOT], McpServiceSecretPresence::NotStored);
    assert!(before.witnesses.is_empty());
    let witness = written(&store, &reference("team", &defined), "sk-hidden").await;
    let after = custody.observe(&defined).await.expect("observed");
    assert_eq!(after.slots[SLOT], McpServiceSecretPresence::Stored);
    // The slot's current write is reported, never the value.
    assert_eq!(after.witnesses.get(SLOT), Some(&witness));
    assert!(!format!("{after:?}").contains("sk-hidden"));
}

/// A launch records the write of each value it read; a recheck answers from
/// the realm documents and the store, read now, whether that write is still
/// what a launch of its server would inject.
#[tokio::test]
async fn a_recheck_answers_whether_a_write_is_still_what_a_launch_injects() {
    use McpServiceSecretRecheck::{Current, DefinitionChanged, Removed, Rotated};
    let defined = search(&["--stdio"]);
    let docs = Documents::new([
        ("global", document("global", None, vec![defined.clone()])),
        (
            "child",
            document("child", Some(RealmId::global()), Vec::new()),
        ),
    ]);
    let store = store();
    let custody = RealmMcpServiceSecrets::from_realm_source(
        Arc::clone(&docs) as Arc<dyn meerkat_core::RealmConfigSource>,
        realm("child"),
        Arc::clone(&store),
    );
    let global = reference("global", &defined);

    // The write a launch reads is the one the observation reports.
    let first = written(&store, &global, "v1").await;
    let launched = custody.resolve(&defined).await.expect("launch values");
    assert_eq!(launched.launch().witness(SLOT), Some(&first));
    assert_eq!(
        custody
            .observe(&defined)
            .await
            .expect("observed")
            .witnesses
            .get(SLOT),
        Some(&first)
    );
    assert_eq!(custody.recheck(&first).await, Ok(Current));

    // A rewrite, even of the same value, is another write: the earlier one
    // is rotated, while the launch that read it keeps its record.
    let second = written(&store, &global, "v1").await;
    assert_ne!(first, second);
    assert_eq!(custody.recheck(&first).await, Ok(Rotated));
    assert_eq!(custody.recheck(&second).await, Ok(Current));
    assert_eq!(launched.launch().witness(SLOT), Some(&first));
    assert_eq!(
        custody
            .resolve(&defined)
            .await
            .expect("launch values")
            .launch()
            .witness(SLOT),
        Some(&second)
    );

    // A removal.
    clear_mcp_service_secret(
        &meerkat_core::AdmissionGuard::none(),
        store.as_ref(),
        &global,
    )
    .await
    .unwrap();
    assert_eq!(custody.recheck(&second).await, Ok(Removed));
    assert_eq!(custody.recheck(&first).await, Ok(Removed));

    // Another realm's definition now serves the launches: a child repeating
    // the definition defines it itself, so the parent's write is no longer
    // what a launch injects, and is again once the child's is gone.
    let third = written(&store, &global, "v3").await;
    docs.replace(
        "child",
        document("child", Some(RealmId::global()), vec![defined.clone()]),
    );
    assert_eq!(custody.recheck(&third).await, Ok(DefinitionChanged));
    docs.replace(
        "child",
        document("child", Some(RealmId::global()), Vec::new()),
    );
    assert_eq!(custody.recheck(&third).await, Ok(Current));

    // A changed definition, then none at all.
    docs.replace(
        "global",
        document("global", None, vec![search(&["--other"])]),
    );
    assert_eq!(custody.recheck(&third).await, Ok(DefinitionChanged));
    docs.replace("global", document("global", None, Vec::new()));
    assert_eq!(custody.recheck(&third).await, Ok(DefinitionChanged));

    // An identical reinstallation is the same reference (the same identity
    // an obsolete cleanup keeps), so its stored write answers again.
    docs.replace("global", document("global", None, vec![defined.clone()]));
    assert_eq!(custody.recheck(&third).await, Ok(Current));

    // A value that names no write (a host's own) is recorded without one.
    let mut host = meerkat_core::auth::McpServiceSecretValues::new();
    host.insert(SLOT, "host-value");
    assert_eq!(host.launch().witness(SLOT), None);
}

/// An explicit credential source with answers of its own for every method.
#[cfg(feature = "mcp")]
struct Explicit {
    egress: meerkat_mcp::HttpEgressProfile,
    custody: Option<Result<meerkat_core::auth::McpServiceSecretValues, McpServiceSecretRefusal>>,
}

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl meerkat_mcp::McpAuthResolver for Explicit {
    async fn stored_bearer_token(
        &self,
        _target: &meerkat_providers::mcp_oauth::McpServerIdentity,
    ) -> Result<Option<String>, meerkat_providers::mcp_oauth::McpOAuthError> {
        Ok(Some("explicit-token".to_owned()))
    }

    async fn interactive_login(
        &self,
        _target: &meerkat_providers::mcp_oauth::McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<String, meerkat_providers::mcp_oauth::McpOAuthError> {
        Ok(format!("login:{}", www_authenticate.unwrap_or_default()))
    }

    fn credential_realm(&self) -> Option<RealmId> {
        Some(realm("explicit"))
    }

    fn http_egress(&self) -> Option<meerkat_mcp::HttpEgressProfile> {
        Some(self.egress.clone())
    }

    async fn stdio_service_secrets(
        &self,
        server: &McpServerConfig,
    ) -> Result<meerkat_core::auth::McpServiceSecretValues, McpServiceSecretRefusal> {
        match &self.custody {
            Some(answer) => answer.clone(),
            None => {
                let _ = server;
                Err(McpServiceSecretRefusal::NoCustody)
            }
        }
    }

    fn credential_publication(
        &self,
        _target: &meerkat_providers::mcp_oauth::McpServerIdentity,
    ) -> Option<meerkat_core::handles::CredentialLeasePublication> {
        Some(explicit_publication())
    }
}

/// The lease publication the explicit credential source states.
#[cfg(feature = "mcp")]
fn explicit_publication() -> meerkat_core::handles::CredentialLeasePublication {
    meerkat_core::handles::CredentialLeasePublication::of(
        meerkat_core::handles::LeaseKey::new(
            realm("explicit"),
            meerkat_core::BindingId::parse("explicit-source").unwrap(),
            None,
        ),
        &meerkat_core::handles::AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            credential_present: true,
            generation: 7,
            credential_published_at_millis: None,
            adopted_marker_generation: None,
            released_publication_at_millis: None,
        },
    )
    .expect("a lease that holds a credential has a publication")
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn the_custody_resolver_forwards_every_credential_method_and_keeps_explicit_custody() {
    let defined = search(&[]);
    let docs = Documents::new([("team", document("team", None, vec![defined.clone()]))]);
    let store = store();
    let witness = written(&store, &reference("team", &defined), "realm-value").await;
    let custody = RealmMcpServiceSecrets::from_realm_source(
        docs as Arc<dyn meerkat_core::RealmConfigSource>,
        realm("team"),
        store,
    );
    let egress = meerkat_mcp::HttpEgressProfile::required(meerkat_mcp::HttpEgressPolicy::new());
    let identity = meerkat_providers::mcp_oauth::McpServerIdentity::from_config(
        &McpServerConfig::streamable_http("remote", "https://mcp.example.com/mcp", HashMap::new()),
    )
    .expect("identity");

    // Without custody of its own, the explicit source gets the realm's.
    let wrapped = custody.clone().serve_for(Arc::new(Explicit {
        egress: egress.clone(),
        custody: None,
    }));
    assert_eq!(
        wrapped.stored_bearer_token(&identity).await.unwrap(),
        Some("explicit-token".to_owned())
    );
    assert_eq!(
        wrapped
            .interactive_login(&identity, Some("challenge"))
            .await
            .unwrap(),
        "login:challenge"
    );
    assert_eq!(wrapped.credential_realm(), Some(realm("explicit")));
    assert!(
        wrapped
            .http_egress()
            .is_some_and(|forwarded| forwarded.same_profile(&egress)),
        "the explicit source's egress profile is forwarded unchanged"
    );
    assert_eq!(
        wrapped.credential_publication(&identity),
        Some(explicit_publication()),
        "the explicit source's lease publication is forwarded unchanged"
    );
    assert_eq!(
        wrapped
            .stdio_service_secrets(&defined)
            .await
            .expect("realm custody")
            .into_env()
            .collect::<Vec<_>>(),
        vec![(SLOT.to_owned(), "realm-value".to_owned())]
    );
    // The realm custody's values carry the write each was read from, which
    // is what the launch records.
    assert_eq!(
        wrapped
            .stdio_service_secrets(&defined)
            .await
            .expect("realm custody")
            .launch()
            .witness(SLOT),
        Some(&witness)
    );

    // An explicit source's own custody answer stands, refusal included.
    let explicit_refusal = Err(McpServiceSecretRefusal::Unavailable {
        reason: "explicit".to_owned(),
    });
    let wrapped = custody.serve_for(Arc::new(Explicit {
        egress,
        custody: Some(explicit_refusal.clone()),
    }));
    assert_eq!(
        wrapped.stdio_service_secrets(&defined).await,
        explicit_refusal
    );
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn the_custody_resolver_keeps_the_account_its_inner_source_selected() {
    use meerkat_core::OperationAccount;
    use meerkat_mcp::McpAuthResolver as _;
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let native = crate::HostAuthService::new(
        meerkat_providers::auth_store::ProviderAuthPersistence::new(
            store(),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        ),
        runtime.provider_auth_runtime_authority(),
    )
    .with_mcp_realm(realm("team"))
    .mcp_oauth_authority()
    .expect("the native MCP OAuth owner");
    let target = |account: Option<&str>| {
        let mut server = McpServerConfig::streamable_http(
            "mail",
            "https://mcp.example:8443/mcp",
            HashMap::new(),
        );
        if let McpTransportConfig::Http(http) = &mut server.transport {
            http.oauth_account = account.map(str::to_owned);
        }
        meerkat_providers::mcp_oauth::McpServerIdentity::from_config(&server)
            .expect("the server's OAuth target")
    };
    let (selected, unselected) = (target(Some("subject-7")), target(None));
    let known = OperationAccount::selected("https://mcp.example:8443", "subject-7");
    assert_eq!(
        native.selected_account(&selected),
        known,
        "positive control: the native owner states its Known selection"
    );

    // The custody states the native owner's selection, and none of its own.
    let docs = Documents::new([("team", document("team", None, vec![search(&[])]))]);
    let wrapped = RealmMcpServiceSecrets::from_realm_source(
        docs as Arc<dyn meerkat_core::RealmConfigSource>,
        realm("team"),
        store(),
    )
    .serve_for(Arc::new(native));
    assert_eq!(wrapped.selected_account(&selected), known);
    assert_eq!(
        wrapped.selected_account(&unselected),
        OperationAccount::Unavailable
    );
}

#[cfg(all(feature = "session-store", feature = "local-authorization"))]
mod owner_updates {
    use std::path::{Path, PathBuf};

    use meerkat_core::auth::load_mcp_service_secret;
    use meerkat_core::storage_layout::ResolvedStorage;
    use meerkat_core::{ConfigRuntime, FileConfigStore, McpRealmPersistTarget};

    use super::*;

    fn storage(root: &Path) -> ResolvedStorage {
        ResolvedStorage {
            layout: meerkat_core::storage_layout::StorageLayout::with_injected_roots(
                root.to_path_buf(),
                None,
                None,
                root.to_path_buf(),
            ),
            locator: meerkat_core::runtime_bootstrap::RealmLocator {
                state_root: root.to_path_buf(),
                realm: realm("team"),
            },
            root_choice: meerkat_core::runtime_bootstrap::RealmRootChoice::Explicit,
        }
    }

    /// The realm's own config document, its directory created.
    fn document_path(storage: &ResolvedStorage) -> PathBuf {
        let document = crate::governed_realm::stock::issuance::realm_config_source(storage)
            .expect("projection")
            .config_doc_path(&storage.locator.realm);
        std::fs::create_dir_all(document.parent().expect("document directory")).unwrap();
        document
    }

    fn runtime(document: &Path) -> ConfigRuntime {
        ConfigRuntime::new(
            Arc::new(FileConfigStore::new(
                document.to_path_buf(),
                meerkat_models::canonical(),
            )),
            document.with_file_name("config_state.json"),
        )
    }

    fn persistence() -> meerkat_providers::auth_store::ProviderAuthPersistence {
        meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        )
    }

    type Observed = Result<
        meerkat_core::auth::CredentialMutationOutcome,
        meerkat_core::auth::CredentialMutationError,
    >;

    /// The in-memory coordinator, keeping each exclusive mutation's result
    /// as the coordinator returns it.
    struct ObservingCoordinator {
        inner: meerkat_providers::auth_store::InMemoryCoordinator,
        observed: std::sync::Mutex<Vec<Observed>>,
    }

    impl ObservingCoordinator {
        fn take(&self) -> Vec<Observed> {
            std::mem::take(&mut *self.observed.lock().unwrap())
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::auth::RefreshCoordinator for ObservingCoordinator {
        async fn with_exclusive_mutation(
            &self,
            key: meerkat_core::auth::TokenKey,
            mutation_fn: meerkat_core::auth::CredentialMutationFn,
        ) -> Observed {
            self.with_exclusive_mutation_for(
                key,
                meerkat_core::auth::TokenStoreAccess::Interactive,
                mutation_fn,
            )
            .await
        }

        async fn with_exclusive_mutation_for(
            &self,
            key: meerkat_core::auth::TokenKey,
            access: meerkat_core::auth::TokenStoreAccess,
            mutation_fn: meerkat_core::auth::CredentialMutationFn,
        ) -> Observed {
            let result = self
                .inner
                .with_exclusive_mutation_for(key, access, mutation_fn)
                .await;
            self.observed.lock().unwrap().push(result.clone());
            result
        }

        async fn with_refresh(
            &self,
            key: meerkat_core::auth::TokenKey,
            refresh_fn: meerkat_core::auth::RefreshFn,
        ) -> Result<meerkat_core::auth::PersistedTokens, meerkat_core::auth::RefreshError> {
            self.inner.with_refresh(key, refresh_fn).await
        }
    }

    /// An ephemeral store whose writes fail after they begin.
    struct FailingWrites(meerkat_providers::auth_store::EphemeralTokenStore);

    #[async_trait::async_trait]
    impl TokenStore for FailingWrites {
        async fn load(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
        {
            self.0.load(key).await
        }
        async fn save(
            &self,
            _fence: &meerkat_core::AdmissionGuard,
            _key: &meerkat_core::auth::TokenKey,
            _tokens: &meerkat_core::auth::PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            Err(meerkat_core::auth::TokenStoreError::Io(
                "the write was interrupted".to_owned(),
            ))
        }
        async fn clear(
            &self,
            _fence: &meerkat_core::AdmissionGuard,
            _key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            Err(meerkat_core::auth::TokenStoreError::Io(
                "the removal was interrupted".to_owned(),
            ))
        }
        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            self.0.list().await
        }
        fn backend_name(&self) -> &'static str {
            "failing-writes"
        }
    }

    /// The realm's own document listing exactly `servers`, written as is.
    fn write_document(document: &Path, servers: Vec<McpServerConfig>) {
        let mut doc = Config::default();
        doc.tools.mcp_servers = servers;
        std::fs::write(document, toml::to_string(&doc).unwrap()).unwrap();
    }

    async fn stored(
        persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
        server: &McpServerConfig,
    ) -> Option<String> {
        load_mcp_service_secret(
            persistence.token_store().as_ref(),
            &reference("team", server),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_value_is_stored_only_for_the_reviewed_current_installation() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let persistence = persistence();
        let reviewed = search(&["--stdio"]);

        // Not installed: refused, nothing stored.
        assert!(matches!(
            set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1").await,
            Err(McpServiceSecretUpdateError::DefinitionChanged(_))
        ));
        assert_eq!(stored(&persistence, &reviewed).await, None);

        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        let witness = set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
            .await
            .expect("stored for the current installation");
        assert_eq!(witness.reference().realm(), &realm("team"));
        assert_eq!(witness.reference(), &reference("team", &reviewed));
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v1"));

        // The launch custody reads exactly what the owner stored.
        let custody = RealmMcpServiceSecrets::from_realm_source(
            Arc::new(
                crate::governed_realm::stock::issuance::realm_config_source(&storage)
                    .expect("projection"),
            ),
            realm("team"),
            persistence.token_store(),
        );
        assert_eq!(
            custody
                .resolve(&reviewed)
                .await
                .expect("launch value")
                .into_env()
                .collect::<Vec<_>>(),
            vec![(SLOT.to_owned(), "v1".to_owned())]
        );

        // A same-name replacement is another reference: a write for the
        // reviewed definition is refused, and the replacement never reads
        // the reviewed value.
        let replacement = search(&["--replacement"]);
        let config = runtime(&document);
        let target = McpRealmPersistTarget::new(&config);
        target.remove("search", None).await.unwrap();
        target.add(replacement.clone(), None).await.unwrap();
        assert!(matches!(
            set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v2").await,
            Err(McpServiceSecretUpdateError::DefinitionChanged(_))
        ));
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v1"));
        assert_eq!(
            custody.resolve(&replacement).await,
            Err(McpServiceSecretRefusal::NotStored {
                env: SLOT.to_owned()
            })
        );

        // Cleanup of the replaced definition removes its value; a current
        // removal of it is refused as changed.
        assert!(matches!(
            remove_mcp_service_secret(
                &storage,
                &persistence,
                &reviewed,
                SLOT,
                McpServiceSecretRemoval::Current
            )
            .await,
            Err(McpServiceSecretUpdateError::DefinitionChanged(_))
        ));
        remove_mcp_service_secret(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Obsolete,
        )
        .await
        .expect("obsolete value removed");
        assert_eq!(stored(&persistence, &reviewed).await, None);
    }

    #[tokio::test]
    async fn a_delayed_cleanup_never_removes_a_current_identical_reinstallation() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let persistence = persistence();
        let reviewed = search(&[]);
        let config = runtime(&document);
        McpRealmPersistTarget::new(&config)
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "fresh")
            .await
            .unwrap();

        // An identical reinstallation is current: the obsolete cleanup is
        // refused and the value stays.
        assert!(matches!(
            remove_mcp_service_secret(
                &storage,
                &persistence,
                &reviewed,
                SLOT,
                McpServiceSecretRemoval::Obsolete
            )
            .await,
            Err(McpServiceSecretUpdateError::StillCurrent(_))
        ));
        assert_eq!(
            stored(&persistence, &reviewed).await.as_deref(),
            Some("fresh")
        );
        // A deliberate current removal is distinct and succeeds.
        remove_mcp_service_secret(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
        )
        .await
        .expect("current value removed");
        assert_eq!(stored(&persistence, &reviewed).await, None);

        // A document listing the name twice installs its later entry, as the
        // composition makes it effective: that entry is current, and a
        // cleanup of it never inspects the earlier one.
        let shadowed = search(&["--shadowed"]);
        write_document(&document, vec![shadowed.clone(), reviewed.clone()]);
        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "effective")
            .await
            .expect("the effective entry is current");
        assert!(matches!(
            remove_mcp_service_secret(
                &storage,
                &persistence,
                &reviewed,
                SLOT,
                McpServiceSecretRemoval::Obsolete
            )
            .await,
            Err(McpServiceSecretUpdateError::StillCurrent(_))
        ));
        assert_eq!(
            stored(&persistence, &reviewed).await.as_deref(),
            Some("effective")
        );
        assert!(matches!(
            set_mcp_service_secret(&storage, &persistence, &shadowed, SLOT, "shadowed").await,
            Err(McpServiceSecretUpdateError::DefinitionChanged(_))
        ));
        remove_mcp_service_secret(
            &storage,
            &persistence,
            &shadowed,
            SLOT,
            McpServiceSecretRemoval::Obsolete,
        )
        .await
        .expect("the shadowed entry is not current");
        assert_eq!(
            stored(&persistence, &reviewed).await.as_deref(),
            Some("effective")
        );
        // The launch custody agrees with the owner: the effective entry
        // reads its value, the shadowed one is not current.
        let custody = RealmMcpServiceSecrets::from_realm_source(
            Arc::new(
                crate::governed_realm::stock::issuance::realm_config_source(&storage)
                    .expect("projection"),
            ),
            realm("team"),
            persistence.token_store(),
        );
        assert_eq!(
            custody
                .resolve(&reviewed)
                .await
                .expect("launch value")
                .into_env()
                .collect::<Vec<_>>(),
            vec![(SLOT.to_owned(), "effective".to_owned())]
        );
        assert_eq!(
            custody.resolve(&shadowed).await,
            Err(McpServiceSecretRefusal::DefinitionChanged)
        );
    }

    #[tokio::test]
    async fn the_mutation_owner_receives_the_durable_result_of_each_update() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let reviewed = search(&[]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        let key = meerkat_core::auth::TokenKey::mcp_service_secret(&reference("team", &reviewed));
        let coordinator = Arc::new(ObservingCoordinator {
            inner: meerkat_providers::auth_store::InMemoryCoordinator::new(),
            observed: std::sync::Mutex::new(Vec::new()),
        });
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            Arc::clone(&coordinator) as Arc<dyn meerkat_core::auth::RefreshCoordinator>,
        );

        // A set: the owner receives exactly the row the store persisted.
        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
            .await
            .unwrap();
        let persisted = persistence
            .token_store()
            .load(&key)
            .await
            .unwrap()
            .expect("stored row");
        assert!(matches!(
            coordinator.take().as_slice(),
            [Ok(meerkat_core::auth::CredentialMutationOutcome::Persisted(tokens))]
                if *tokens == persisted
        ));

        // A refused update: an error, and nothing written.
        assert!(matches!(
            remove_mcp_service_secret(
                &storage,
                &persistence,
                &reviewed,
                SLOT,
                McpServiceSecretRemoval::Obsolete
            )
            .await,
            Err(McpServiceSecretUpdateError::StillCurrent(_))
        ));
        assert!(matches!(
            coordinator.take().as_slice(),
            [Err(meerkat_core::auth::CredentialMutationError::Operation(
                _
            ))]
        ));
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v1"));

        // A removal: the owner receives the durable absence.
        remove_mcp_service_secret(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
        )
        .await
        .unwrap();
        assert!(matches!(
            coordinator.take().as_slice(),
            [Ok(meerkat_core::auth::CredentialMutationOutcome::Cleared)]
        ));
        assert_eq!(stored(&persistence, &reviewed).await, None);

        // A write that began and failed: the owner receives the store's
        // error, never a success, and the caller learns the outcome is
        // unknown.
        let failing = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(FailingWrites(
                meerkat_providers::auth_store::EphemeralTokenStore::new(),
            )),
            Arc::clone(&coordinator) as Arc<dyn meerkat_core::auth::RefreshCoordinator>,
        );
        assert!(matches!(
            set_mcp_service_secret(&storage, &failing, &reviewed, SLOT, "v2").await,
            Err(McpServiceSecretUpdateError::OutcomeUnknown(_))
        ));
        assert!(matches!(
            coordinator.take().as_slice(),
            [Err(
                meerkat_core::auth::CredentialMutationError::TokenStore(_)
            )]
        ));
        assert!(matches!(
            remove_mcp_service_secret(
                &storage,
                &failing,
                &reviewed,
                SLOT,
                McpServiceSecretRemoval::Current
            )
            .await,
            Err(McpServiceSecretUpdateError::OutcomeUnknown(_))
        ));
        assert!(matches!(
            coordinator.take().as_slice(),
            [Err(
                meerkat_core::auth::CredentialMutationError::TokenStore(_)
            )]
        ));
    }

    #[tokio::test]
    async fn a_config_write_in_progress_refuses_the_update_with_nothing_written() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let persistence = persistence();
        let reviewed = search(&[]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        let held = meerkat_core::config_runtime::ConfigWriterLock::try_hold(
            &document.with_file_name("config_state.lock"),
        )
        .unwrap()
        .expect("config writer custody");
        assert!(matches!(
            set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1").await,
            Err(McpServiceSecretUpdateError::ConfigurationUnavailable(_))
        ));
        assert_eq!(stored(&persistence, &reviewed).await, None);
        drop(held);
        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
            .await
            .expect("stored once the write settled");
    }

    /// The realm document's shared custody, the one a currentness read holds
    /// (`ConfigReadLock`), excludes every service-secret update as a held
    /// writer custody does: an update needs the exclusive custody and takes
    /// it without waiting. While it is held, a rotation and a removal are
    /// both refused as `ConfigurationUnavailable` with nothing written; once
    /// it is released, both land.
    #[tokio::test]
    async fn a_held_read_custody_refuses_a_secret_update_with_nothing_written() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let persistence = persistence();
        let reviewed = search(&[]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
            .await
            .expect("stored before any custody is held");
        let held = meerkat_core::config_runtime::ConfigReadLock::try_hold(
            &document.with_file_name("config_state.lock"),
        )
        .unwrap()
        .expect("config read custody");

        assert!(matches!(
            set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v2").await,
            Err(McpServiceSecretUpdateError::ConfigurationUnavailable(_))
        ));
        assert!(matches!(
            remove_mcp_service_secret(
                &storage,
                &persistence,
                &reviewed,
                SLOT,
                McpServiceSecretRemoval::Current
            )
            .await,
            Err(McpServiceSecretUpdateError::ConfigurationUnavailable(_))
        ));
        assert_eq!(
            stored(&persistence, &reviewed).await.as_deref(),
            Some("v1"),
            "nothing was written while the read custody was held"
        );

        drop(held);
        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v2")
            .await
            .expect("the rotation lands once the read custody is released");
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v2"));
        remove_mcp_service_secret(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
        )
        .await
        .expect("the removal lands once the read custody is released");
        assert_eq!(stored(&persistence, &reviewed).await, None);
    }

    /// A durable credential store under `root` and its fenced coordinator,
    /// retaining custody: an interactive update waits at most 5 s for a held
    /// credential fence, then refuses.
    fn fenced_persistence(root: &Path) -> meerkat_providers::auth_store::ProviderAuthPersistence {
        meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::FileTokenStore::new(root)),
            Arc::new(
                meerkat_providers::auth_store::FileLockCoordinator::retaining_custody(
                    root.join("refresh-locks"),
                ),
            ),
        )
    }

    /// An admission holder's hold of C(`team`) under the credential root
    /// `root`: SHARED, so every fenced credential write of the realm is
    /// excluded until it is released.
    fn hold_team_credentials(root: &Path) -> meerkat_core::AdmissionHold {
        let fence = meerkat_core::AdmissionFence::credentials(root, &realm("team")).unwrap();
        fence.ensure_exists().unwrap();
        meerkat_core::AdmissionHold::try_acquire(vec![fence]).unwrap()
    }

    /// The update takes the credential fence once, at its coordinator entry;
    /// the custodies nested inside it (the config document's writer
    /// exclusion, the write) take none, so with no holder a set and a
    /// removal both land.
    #[tokio::test]
    async fn with_no_holder_a_secret_write_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let persistence = fenced_persistence(&temp.path().join("credentials"));
        let reviewed = search(&[]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();

        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
            .await
            .expect("stored with no holder");
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v1"));
        remove_mcp_service_secret(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
        )
        .await
        .expect("removed with no holder");
        assert_eq!(stored(&persistence, &reviewed).await, None);
    }

    /// While an admission holds the realm's credential fence, a rotation and
    /// a removal are refused as `AdmissionInProgress` with nothing written,
    /// and land once it is released.
    #[tokio::test]
    async fn under_a_held_fence_a_secret_write_refuses_admission_in_progress_with_nothing_written()
    {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let credentials = temp.path().join("credentials");
        let persistence = fenced_persistence(&credentials);
        let reviewed = search(&[]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
            .await
            .expect("stored before the hold");

        let held = hold_team_credentials(&credentials);
        let rotated = set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v2").await;
        assert!(
            matches!(
                rotated,
                Err(McpServiceSecretUpdateError::AdmissionInProgress)
            ),
            "{rotated:?}"
        );
        let removed = remove_mcp_service_secret(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
        )
        .await;
        assert!(
            matches!(
                removed,
                Err(McpServiceSecretUpdateError::AdmissionInProgress)
            ),
            "{removed:?}"
        );
        assert_eq!(
            stored(&persistence, &reviewed).await.as_deref(),
            Some("v1"),
            "nothing was written while the fence was held"
        );

        drop(held);
        set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v2")
            .await
            .expect("the rotation lands once the holder released");
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v2"));
    }

    #[tokio::test]
    async fn invalid_updates_touch_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let persistence = persistence();
        let reviewed = search(&[]);
        assert!(matches!(
            set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "").await,
            Err(McpServiceSecretUpdateError::Invalid(_))
        ));
        assert!(matches!(
            set_mcp_service_secret(&storage, &persistence, &reviewed, "UNDECLARED", "v").await,
            Err(McpServiceSecretUpdateError::Invalid(_))
        ));
        let rendered = format!(
            "{:?}",
            set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "sk-never-shown").await
        );
        assert!(!rendered.contains("sk-never-shown"), "{rendered}");
    }

    /// A host's entry check in a test: admits while `admin` holds and counts
    /// its calls. While an admission it granted is held, `held` is set.
    struct HostEntry {
        admin: std::sync::atomic::AtomicBool,
        calls: std::sync::atomic::AtomicUsize,
        held: Arc<std::sync::atomic::AtomicBool>,
    }

    impl HostEntry {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                admin: std::sync::atomic::AtomicBool::new(true),
                calls: std::sync::atomic::AtomicUsize::new(0),
                held: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            })
        }

        fn set_admin(&self, admin: bool) {
            self.admin.store(admin, std::sync::atomic::Ordering::SeqCst);
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn held(&self) -> bool {
            self.held.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Clears the entry's `held` flag when the update releases the admission.
    struct HeldUntilReleased(Arc<std::sync::atomic::AtomicBool>);

    impl Drop for HeldUntilReleased {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl meerkat_core::AdministrationEntry for HostEntry {
        fn enter(
            &self,
        ) -> Result<meerkat_core::EnteredAdministration, meerkat_core::AdministrationEntryRefusal>
        {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if !self.admin.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(meerkat_core::AdministrationEntryRefusal::NotAdministrator);
            }
            self.held.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(meerkat_core::EnteredAdministration::holding(
                HeldUntilReleased(Arc::clone(&self.held)),
            ))
        }
    }

    /// An ephemeral store recording, at each write, whether the update's
    /// admission was held.
    struct WritesUnderAdmission {
        inner: meerkat_providers::auth_store::EphemeralTokenStore,
        held: Arc<std::sync::atomic::AtomicBool>,
        writes_while_held: std::sync::Mutex<Vec<bool>>,
    }

    impl WritesUnderAdmission {
        fn record(&self) {
            self.writes_while_held
                .lock()
                .unwrap()
                .push(self.held.load(std::sync::atomic::Ordering::SeqCst));
        }
    }

    #[async_trait::async_trait]
    impl TokenStore for WritesUnderAdmission {
        async fn load(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.load(key).await
        }
        async fn save(
            &self,
            fence: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
            tokens: &meerkat_core::auth::PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.record();
            self.inner.save(fence, key, tokens).await
        }
        async fn clear(
            &self,
            fence: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.record();
            self.inner.clear(fence, key).await
        }
        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.list().await
        }
        fn backend_name(&self) -> &'static str {
            "writes-under-admission"
        }
    }

    fn value(text: &str) -> meerkat_core::auth::McpServiceSecretValue {
        meerkat_core::auth::McpServiceSecretValue::new(text.to_owned())
    }

    /// An entered update checks the caller only after the installation
    /// check, at the write's entry; a refusal there writes nothing, and an
    /// admitted write runs while the admission is held, which the update
    /// releases only once it has settled.
    #[tokio::test]
    async fn an_entered_update_is_admitted_at_its_entry_and_held_until_it_settles() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let reviewed = search(&["--stdio"]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        let entry = HostEntry::new();
        let store = Arc::new(WritesUnderAdmission {
            inner: meerkat_providers::auth_store::EphemeralTokenStore::new(),
            held: Arc::clone(&entry.held),
            writes_while_held: std::sync::Mutex::new(Vec::new()),
        });
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::clone(&store) as Arc<dyn TokenStore>,
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let entered = Arc::clone(&entry) as Arc<dyn meerkat_core::AdministrationEntry>;

        // A definition the realm does not list is refused before the entry.
        assert!(matches!(
            set_mcp_service_secret_entered(
                &storage,
                &persistence,
                &search(&["--changed"]),
                SLOT,
                value("v1"),
                Arc::clone(&entered),
            )
            .await,
            Err(McpServiceSecretUpdateError::DefinitionChanged(_))
        ));
        assert_eq!(entry.calls(), 0);

        // Refused at the entry: nothing is written.
        entry.set_admin(false);
        assert!(matches!(
            set_mcp_service_secret_entered(
                &storage,
                &persistence,
                &reviewed,
                SLOT,
                value("v1"),
                Arc::clone(&entered),
            )
            .await,
            Err(McpServiceSecretUpdateError::NotAdmitted(
                meerkat_core::AdministrationEntryRefusal::NotAdministrator
            ))
        ));
        assert_eq!(entry.calls(), 1);
        assert_eq!(stored(&persistence, &reviewed).await, None);
        assert!(store.writes_while_held.lock().unwrap().is_empty());

        // Admitted: written while the admission is held, released after.
        entry.set_admin(true);
        set_mcp_service_secret_entered(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            value("v1"),
            Arc::clone(&entered),
        )
        .await
        .expect("admitted");
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v1"));
        assert_eq!(*store.writes_while_held.lock().unwrap(), vec![true]);
        assert!(!entry.held(), "released once the update settled");

        // A removal is admitted at its own entry the same way.
        entry.set_admin(false);
        assert!(matches!(
            remove_mcp_service_secret_entered(
                &storage,
                &persistence,
                &reviewed,
                SLOT,
                McpServiceSecretRemoval::Current,
                Arc::clone(&entered),
            )
            .await,
            Err(McpServiceSecretUpdateError::NotAdmitted(_))
        ));
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v1"));
        entry.set_admin(true);
        remove_mcp_service_secret_entered(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
            entered,
        )
        .await
        .expect("admitted removal");
        assert_eq!(stored(&persistence, &reviewed).await, None);
        assert_eq!(*store.writes_while_held.lock().unwrap(), vec![true, true]);
        assert!(!entry.held());
    }

    /// A caller that loses its administration while its update waits for
    /// the credential mutation owner of the exact reference is refused at
    /// the update's entry, with nothing written: a check before the wait
    /// would have admitted it.
    #[tokio::test]
    async fn a_caller_revoked_while_its_update_waits_is_refused_at_entry() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let reviewed = search(&["--stdio"]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        let persistence = persistence();
        let entry = HostEntry::new();

        // Another update holds the credential mutation owner of the reference.
        let key = meerkat_core::auth::TokenKey::mcp_service_secret(&reference("team", &reviewed));
        let (holding, held) = tokio::sync::oneshot::channel::<()>();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let coordinator = persistence.refresh_coordinator();
        let holder = tokio::spawn(async move {
            coordinator
                .with_exclusive_mutation_for(
                    key,
                    meerkat_core::auth::TokenStoreAccess::Interactive,
                    Box::new(move |_fence| {
                        Box::pin(async move {
                            let _ = holding.send(());
                            let _ = released.await;
                            Ok(meerkat_core::auth::CredentialMutationOutcome::Cleared)
                        })
                    }),
                )
                .await
        });
        held.await.unwrap();
        let update = tokio::spawn({
            let (storage, persistence) = (storage.clone(), persistence.clone());
            let (reviewed, entry) = (
                reviewed.clone(),
                Arc::clone(&entry) as Arc<dyn meerkat_core::AdministrationEntry>,
            );
            async move {
                set_mcp_service_secret_entered(
                    &storage,
                    &persistence,
                    &reviewed,
                    SLOT,
                    value("v1"),
                    entry,
                )
                .await
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!update.is_finished(), "the update waits for the custody");
        assert_eq!(entry.calls(), 0, "nothing is admitted before the custody");

        // The caller loses its administration while the update waits.
        entry.set_admin(false);
        release.send(()).unwrap();
        holder.await.unwrap().unwrap();
        assert!(matches!(
            update.await.unwrap(),
            Err(McpServiceSecretUpdateError::NotAdmitted(_))
        ));
        assert_eq!(entry.calls(), 1);
        assert_eq!(stored(&persistence, &reviewed).await, None);
    }

    /// The records a host's administration recorder received.
    #[derive(Default)]
    struct RecordedAdministrations(std::sync::Mutex<Vec<meerkat_core::AdministrationRecord>>);

    impl RecordedAdministrations {
        fn take(&self) -> Vec<meerkat_core::AdministrationRecord> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl meerkat_core::AdministrationRecorder for RecordedAdministrations {
        fn record(&self, record: meerkat_core::AdministrationRecord) {
            self.0.lock().unwrap().push(record);
        }
    }

    /// The administrator a host authenticated, in its own trust domain.
    fn administrator() -> meerkat_core::PrincipalRef {
        meerkat_core::PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::Human,
            "admin-7",
            meerkat_core::TrustDomainId::new("example-idp").expect("domain"),
        )
        .expect("qualified principal")
    }

    /// A host's entry check that admits its authenticated administrator,
    /// logged in at `login`, and states them on the admission with the
    /// host's recorder.
    struct StatingEntry {
        recorder: Arc<RecordedAdministrations>,
        login: &'static str,
    }

    impl meerkat_core::AdministrationEntry for StatingEntry {
        fn enter(
            &self,
        ) -> Result<meerkat_core::EnteredAdministration, meerkat_core::AdministrationEntryRefusal>
        {
            let administrator = meerkat_core::AdministeringPrincipal::new(administrator())
                .expect("qualified administrator")
                .with_authentication_scope(
                    &meerkat_core::approval::consent::HostAuthenticationScope::new(self.login),
                );
            Ok(
                meerkat_core::EnteredAdministration::admitted().administered_by(
                    administrator,
                    Arc::clone(&self.recorder) as Arc<dyn meerkat_core::AdministrationRecorder>,
                ),
            )
        }
    }

    fn stating(
        recorder: &Arc<RecordedAdministrations>,
        login: &'static str,
    ) -> Arc<dyn meerkat_core::AdministrationEntry> {
        Arc::new(StatingEntry {
            recorder: Arc::clone(recorder),
            login,
        })
    }

    fn login_digest(login: &str) -> meerkat_core::approval::decision::Sha256Digest {
        meerkat_core::approval::consent::HostAuthenticationScope::new(login).digest()
    }

    /// Each update the host admitted with a stated administrator settles
    /// one typed record to the recorder on its admission: the administrator,
    /// the digest of their login, the exact slot set or removed, and how it
    /// ended (committed, or unknown for a write that began and failed). A
    /// refused update records nothing, and no record carries the value.
    #[tokio::test]
    async fn administration_admission_settles_to_the_host_recorder() {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let reviewed = search(&["--stdio"]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        let persistence = persistence();
        let recorder = Arc::new(RecordedAdministrations::default());
        let slot = reference("team", &reviewed);
        let set_change = meerkat_core::AdministeredChange::McpServiceSecretSet {
            realm: realm("team"),
            server: "search".to_owned(),
            env: SLOT.to_owned(),
            meaning: slot.meaning().to_owned(),
        };

        // An admitted set.
        set_mcp_service_secret_entered(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            value("secret-value-canary"),
            stating(&recorder, "login-1"),
        )
        .await
        .expect("admitted set");
        let records = recorder.take();
        assert_eq!(records.len(), 1, "one record per admission");
        let record = &records[0];
        assert_eq!(record.administrator, administrator());
        assert_eq!(
            record.authentication_scope_digest,
            Some(login_digest("login-1"))
        );
        assert_eq!(record.change.as_ref(), Some(&set_change));
        assert_eq!(
            record.outcome,
            meerkat_core::AdministrationOutcome::Committed
        );
        assert!(record.admitted_at <= record.settled_at);
        let rendered = serde_json::to_string(record).expect("encode");
        assert!(
            !rendered.contains("secret-value-canary") && !rendered.contains("login-1"),
            "neither the value nor the login reaches the record: {rendered}"
        );

        // An admitted removal.
        remove_mcp_service_secret_entered(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
            stating(&recorder, "login-2"),
        )
        .await
        .expect("admitted removal");
        let records = recorder.take();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].change,
            Some(meerkat_core::AdministeredChange::McpServiceSecretRemove {
                realm: realm("team"),
                server: "search".to_owned(),
                env: SLOT.to_owned(),
                meaning: slot.meaning().to_owned(),
            })
        );
        assert_eq!(
            records[0].outcome,
            meerkat_core::AdministrationOutcome::Committed
        );
        assert_eq!(
            records[0].authentication_scope_digest,
            Some(login_digest("login-2"))
        );

        // A refused update admitted nothing and records nothing.
        let refusing = HostEntry::new();
        refusing.set_admin(false);
        assert!(matches!(
            set_mcp_service_secret_entered(
                &storage,
                &persistence,
                &reviewed,
                SLOT,
                value("v2"),
                refusing as Arc<dyn meerkat_core::AdministrationEntry>,
            )
            .await,
            Err(McpServiceSecretUpdateError::NotAdmitted(_))
        ));
        assert!(recorder.take().is_empty());

        // A write that began after its admission and failed: recorded, its
        // outcome unknown.
        let failing = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(FailingWrites(
                meerkat_providers::auth_store::EphemeralTokenStore::new(),
            )),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        assert!(matches!(
            set_mcp_service_secret_entered(
                &storage,
                &failing,
                &reviewed,
                SLOT,
                value("v3"),
                stating(&recorder, "login-3"),
            )
            .await,
            Err(McpServiceSecretUpdateError::OutcomeUnknown(_))
        ));
        let records = recorder.take();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].change.as_ref(), Some(&set_change));
        assert_eq!(
            records[0].outcome,
            meerkat_core::AdministrationOutcome::Unknown
        );
    }

    /// An ephemeral store whose next write signals when it began and then
    /// waits to be released.
    struct GatedWrite {
        inner: meerkat_providers::auth_store::EphemeralTokenStore,
        began: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[async_trait::async_trait]
    impl TokenStore for GatedWrite {
        async fn load(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.load(key).await
        }
        async fn save(
            &self,
            fence: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
            tokens: &meerkat_core::auth::PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            let began = self.began.lock().unwrap().take();
            if let Some(began) = began {
                let _ = began.send(());
            }
            let release = self.release.lock().unwrap().take();
            if let Some(release) = release {
                let _ = release.await;
            }
            self.inner.save(fence, key, tokens).await
        }
        async fn clear(
            &self,
            fence: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.inner.clear(fence, key).await
        }
        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.list().await
        }
        fn backend_name(&self) -> &'static str {
            "gated-write"
        }
    }

    /// A caller that stops waiting after its update was admitted does not
    /// lose the record: the credential mutation owner finishes the write,
    /// and the record arrives when it settles, committed.
    #[tokio::test]
    async fn administration_admission_settles_to_the_host_recorder_after_the_caller_stopped_waiting()
     {
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let reviewed = search(&["--stdio"]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();
        let (began, on_began) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(GatedWrite {
                inner: meerkat_providers::auth_store::EphemeralTokenStore::new(),
                began: std::sync::Mutex::new(Some(began)),
                release: std::sync::Mutex::new(Some(released)),
            }),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let recorder = Arc::new(RecordedAdministrations::default());
        let caller = tokio::spawn({
            let (storage, persistence) = (storage.clone(), persistence.clone());
            let (reviewed, entry) = (reviewed.clone(), stating(&recorder, "login-1"));
            async move {
                set_mcp_service_secret_entered(
                    &storage,
                    &persistence,
                    &reviewed,
                    SLOT,
                    value("v1"),
                    entry,
                )
                .await
            }
        });

        // The admitted write began; its caller stops waiting.
        on_began.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(
            recorder.take().is_empty(),
            "nothing is recorded before the update settles"
        );

        // The owner finishes the write, and the record arrives.
        release.send(()).unwrap();
        let records = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let records = recorder.take();
                if !records.is_empty() {
                    return records;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the settled update is recorded");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].administrator, administrator());
        assert_eq!(
            records[0].outcome,
            meerkat_core::AdministrationOutcome::Committed
        );
        assert!(matches!(
            records[0].change,
            Some(meerkat_core::AdministeredChange::McpServiceSecretSet { .. })
        ));
        assert_eq!(stored(&persistence, &reviewed).await.as_deref(), Some("v1"));
    }

    fn realm_custody(
        storage: &ResolvedStorage,
        persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
    ) -> RealmMcpServiceSecrets {
        RealmMcpServiceSecrets::from_realm_source(
            Arc::new(
                crate::governed_realm::stock::issuance::realm_config_source(storage)
                    .expect("projection"),
            ),
            realm("team"),
            persistence.token_store(),
        )
    }

    /// The owner's writes name themselves: a set returns the witness that
    /// the observation reports as current and a launch records, and every
    /// later rewrite, removal, uninstallation or replacement is what a
    /// recheck answers.
    #[tokio::test]
    async fn a_set_returns_the_witness_observed_launched_and_rechecked() {
        use McpServiceSecretRecheck::{Current, DefinitionChanged, Removed, Rotated};
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let persistence = persistence();
        let reviewed = search(&["--stdio"]);
        let config = runtime(&document);
        let target = McpRealmPersistTarget::new(&config);
        target.add(reviewed.clone(), None).await.unwrap();
        let custody = realm_custody(&storage, &persistence);

        let first = set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
            .await
            .expect("stored");
        let observed = custody.observe(&reviewed).await.expect("observed");
        assert_eq!(observed.slots[SLOT], McpServiceSecretPresence::Stored);
        assert_eq!(observed.witnesses.get(SLOT), Some(&first));
        let launched = custody.resolve(&reviewed).await.expect("launch values");
        assert_eq!(launched.launch().witness(SLOT), Some(&first));
        assert_eq!(custody.recheck(&first).await, Ok(Current));

        // Setting the same value again is another write.
        let second = set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
            .await
            .expect("stored");
        assert_ne!(first, second);
        assert_eq!(custody.recheck(&first).await, Ok(Rotated));
        assert_eq!(custody.recheck(&second).await, Ok(Current));
        assert_eq!(launched.launch().witness(SLOT), Some(&first));

        // A removal.
        remove_mcp_service_secret(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
        )
        .await
        .expect("removed");
        assert_eq!(custody.recheck(&second).await, Ok(Removed));
        assert!(
            custody
                .observe(&reviewed)
                .await
                .expect("observed")
                .witnesses
                .is_empty()
        );

        // Uninstalled, then replaced by another definition of the name: the
        // write is no longer what a launch of the server injects, and the
        // replacement's own write is another reference.
        let third = set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v3")
            .await
            .expect("stored");
        target.remove("search", None).await.unwrap();
        assert_eq!(custody.recheck(&third).await, Ok(DefinitionChanged));
        let replacement = search(&["--replacement"]);
        target.add(replacement.clone(), None).await.unwrap();
        assert_eq!(custody.recheck(&third).await, Ok(DefinitionChanged));
        let replaced = set_mcp_service_secret(&storage, &persistence, &replacement, SLOT, "v4")
            .await
            .expect("stored");
        assert_ne!(replaced.reference(), third.reference());
        assert_eq!(custody.recheck(&replaced).await, Ok(Current));

        // Reinstalling exactly the reviewed definition is its reference
        // again: the write still stored for it answers from the store, and
        // the replacement's is the changed one now.
        target.remove("search", None).await.unwrap();
        target.add(reviewed.clone(), None).await.unwrap();
        assert_eq!(custody.recheck(&third).await, Ok(Current));
        assert_eq!(custody.recheck(&replaced).await, Ok(DefinitionChanged));
    }

    /// Rechecks read durable state: after a restart (a new store over the
    /// same credential files and a new custody over the same realm
    /// documents), witnesses a host kept as data get the same answers.
    #[tokio::test]
    async fn a_recheck_after_a_restart_gives_the_same_answers() {
        use McpServiceSecretRecheck::{Current, Removed, Rotated};
        let temp = tempfile::tempdir().unwrap();
        let storage = storage(temp.path());
        let document = document_path(&storage);
        let credentials = temp.path().join("credentials");
        // A fresh store and coordinator over the same credential files, as a
        // restarted process opens them.
        let durable = || fenced_persistence(&credentials);
        let reviewed = search(&["--stdio"]);
        McpRealmPersistTarget::new(&runtime(&document))
            .add(reviewed.clone(), None)
            .await
            .unwrap();

        let (kept_rotated, kept_current, observed) = {
            let persistence = durable();
            let rotated = set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v1")
                .await
                .expect("stored");
            let current = set_mcp_service_secret(&storage, &persistence, &reviewed, SLOT, "v2")
                .await
                .expect("stored");
            let custody = realm_custody(&storage, &persistence);
            assert_eq!(custody.recheck(&rotated).await, Ok(Rotated));
            assert_eq!(custody.recheck(&current).await, Ok(Current));
            (
                serde_json::to_string(&rotated).unwrap(),
                serde_json::to_string(&current).unwrap(),
                custody.observe(&reviewed).await.expect("observed"),
            )
        };
        let persistence = durable();
        let restarted = realm_custody(&storage, &persistence);
        let rotated: McpServiceSecretWitness = serde_json::from_str(&kept_rotated).unwrap();
        let current: McpServiceSecretWitness = serde_json::from_str(&kept_current).unwrap();
        assert_eq!(restarted.recheck(&rotated).await, Ok(Rotated));
        assert_eq!(restarted.recheck(&current).await, Ok(Current));
        assert_eq!(
            restarted.observe(&reviewed).await.expect("observed"),
            observed
        );
        assert_eq!(
            restarted
                .resolve(&reviewed)
                .await
                .expect("launch values")
                .launch()
                .witness(SLOT),
            Some(&current)
        );

        // A removal is durable state too.
        remove_mcp_service_secret(
            &storage,
            &persistence,
            &reviewed,
            SLOT,
            McpServiceSecretRemoval::Current,
        )
        .await
        .expect("removed");
        let restarted = realm_custody(&storage, &durable());
        assert_eq!(restarted.recheck(&current).await, Ok(Removed));
        assert_eq!(restarted.recheck(&rotated).await, Ok(Removed));
    }
}
