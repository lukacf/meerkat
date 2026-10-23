//! The terminal step of a reviewed connector installation, after the review
//! closed: under ONE administrative custody of the realm, recheck what the
//! review approved, persist the exact reviewed MCP server, and issue and
//! select the agent's grant for it.
//!
//! The steps are the realm's existing owners, composed under one custody:
//! the connector account and the realm's effective connector client
//! configuration are re-read and compared with the review; the server is written to the realm's own config
//! document only at the reviewed generation
//! ([`meerkat_core::McpRealmPersistTarget`]); and the grant is issued and
//! selected for that one MCP target as `rkat realm grant --mcp` would, without
//! touching the agent's controller or catalog grants. Nothing here is a
//! registry or selection file of its own, and nothing is retried or guessed:
//! drift or a conflict is a typed refusal before any write.

use std::sync::Arc;

use meerkat_core::connection::CredentialAccountRef;
use meerkat_core::storage_layout::ResolvedStorage;
use meerkat_core::{
    ConfigRuntime, FileConfigStore, McpRealmPersistError, McpRealmPersistTarget, McpServerConfig,
};
use meerkat_providers::connector_login::{
    ConnectorAuthPhase, ConnectorCallExpectation, ConnectorLoginError, ConnectorVerifiedAccount,
};
use meerkat_providers::connector_oauth::ConnectorOAuthRefusal;

use super::StockPrincipals;
use super::issuance::{
    self, McpOperations, StockGrantError, StockGrantOutcome, StockGrantRequest, StockGrantTarget,
};

/// What the closed review approved, exactly as reviewed. It names no secret.
#[derive(Debug, Clone)]
pub struct ReviewedConnector {
    /// The agent the grant is for.
    pub agent: String,
    /// The connector credential slot and the provider account verified for
    /// it at review time.
    pub slot: CredentialAccountRef,
    pub account: ConnectorVerifiedAccount,
    /// The reviewed connector client-auth profile, by its canonical binding
    /// (`<id>@<sha256>`, [`meerkat_providers::connector_login::ConnectorClientAuthProfile::binding`]);
    /// `None` when the server uses the default public-client forms.
    pub client_profile_binding: Option<String>,
    /// The exact MCP server configuration to persist.
    pub server: McpServerConfig,
    /// The generation of the realm's own config document the review read.
    pub expected_generation: u64,
    /// The operations the grant for `server` allows.
    pub operations: McpOperations,
    /// The grant's reviewed lifetime from finalization; `None` is
    /// until-revoked. Issued exactly as reviewed.
    pub expires_in_ms: Option<u64>,
}

/// Why nothing was written: the review no longer holds, or the realm could
/// not be held.
#[derive(Debug, thiserror::Error)]
pub enum ReviewedConnectorRefusal {
    /// The realm is not governed, or its storage or custody is unavailable.
    #[error("the realm's administrative custody is unavailable: {0}")]
    Unavailable(String),
    /// The review itself is not a valid installation (for example a server
    /// name or operations a grant cannot represent).
    #[error("the reviewed installation is not valid: {0}")]
    InvalidReview(String),
    /// The slot no longer holds a credential for the reviewed account.
    #[error("the connector account changed since the review")]
    AccountChanged,
    /// The connector client configuration the review approved changed.
    #[error("the connector client configuration changed since the review")]
    ClientConfigChanged,
    /// The slot's stored credential no longer covers the reviewed server: a
    /// different client, resource or verified context, or a scope an
    /// operation the grant allows requires.
    #[error("the connector credential does not cover the reviewed server: {0}")]
    CredentialMismatch(String),
    /// The realm's config document is no longer at the reviewed generation.
    #[error(
        "the realm configuration changed since the review (generation {current}, reviewed {expected})"
    )]
    ConfigConflict { expected: u64, current: u64 },
    /// The realm already lists a server under the reviewed name.
    #[error("the realm already lists MCP server '{0}'")]
    ServerExists(String),
}

/// The step the finalization reached. Each variant states exactly what was
/// written; nothing is ever rolled back, and an uncertain save never cleans
/// up an earlier selection.
///
/// The grant's own outcomes keep their existing types: issuance uncertainty
/// carries the retained grant id ([`StockGrantError::IssuanceUncertain`]), a
/// selection that was not written carries the issued reference
/// ([`StockGrantError::NotSelected`]), and an uncertain selection carries the
/// issued and the previously selected references
/// ([`StockGrantError::SelectionUncertain`]).
///
/// A completed selection is not an attachment permit or a connection: a new
/// session after the realm reopens binds the agent and attaches the server
/// through the ordinary native checks.
#[derive(Debug)]
pub enum ReviewedConnectorFinalization {
    /// Nothing was written.
    Refused(ReviewedConnectorRefusal),
    /// The server write was attempted and failed after it may have begun:
    /// the realm's config document may or may not list the server. No grant
    /// was issued.
    PersistUncertain { reason: String },
    /// The server was persisted, making the config document's generation
    /// `generation`; `grant` is the outcome of issuing and selecting its
    /// grant, with that owner's own outcomes. An `Err` there is not a
    /// refusal before any write: the server is persisted, and
    /// [`StockGrantError`] says what is known about the grant. In particular
    /// `SelectionUncertain` may mean the selection was written with
    /// uncertain durability.
    Persisted {
        generation: u64,
        grant: Result<StockGrantOutcome, StockGrantError>,
    },
}

/// Finalize a reviewed connector installation in `storage`'s realm as the
/// realm's operator `principals`, under one administrative custody.
///
/// The custody is taken first, and the connector credential owner is built
/// inside it from `factory`'s actual credential persistence and the held
/// realm runtime's provider-auth authority, with the connector client
/// profiles the realm's effective configuration declares now
/// ([`crate::connector_client_auth_profiles`]); no second runtime or
/// prebuilt service is used. Under that custody it rechecks the account,
/// the stored credential against the reviewed server (client, resource,
/// verified context and the scopes the granted operations require), and the
/// client profile; persists `review.server` to the realm's config document
/// at `review.expected_generation`; then issues and selects the agent's
/// grant for that MCP target with the reviewed lifetime, through the same
/// issuance as `rkat realm grant --mcp`: the grant records the reviewed
/// server's connector approval, so a server later bound to another account
/// is not covered by it. Every [`ReviewedConnectorFinalization::Refused`]
/// precedes every write.
///
/// The credential recheck is passive: it loads the slot's persisted tokens
/// (and may rehydrate the slot's auth state from them) but extracts and
/// uses no token, refreshes nothing and enters no call. Its credential
/// lifecycle guard is held only for that recheck and ends when it returns,
/// before the server is written; the realm's administrative custody is
/// separate and holds through the grant. A sign-out after the recheck can
/// leave the installation unusable until the slot is signed in again, but
/// never makes the grant cover another account.
///
/// After the write, the grant is issued and selected on the administrative
/// command's own task (a caller that stops waiting does not drop the
/// outcome), under the config document's writer exclusion taken after the
/// custody and held through the selection write. If the document no longer
/// holds exactly the written server at the generation the write made, the
/// grant is refused locally
/// ([`super::StockGovernanceError::InstallationChanged`]) and nothing is
/// issued.
pub async fn finalize_reviewed_connector(
    storage: &ResolvedStorage,
    principals: &StockPrincipals,
    factory: &crate::AgentFactory,
    review: &ReviewedConnector,
) -> ReviewedConnectorFinalization {
    use ReviewedConnectorFinalization::Refused;
    use ReviewedConnectorRefusal as Refusal;

    let realms_root = storage.locator.state_root.as_path();
    let realm_id = storage.locator.realm.as_str();
    let expectation = match check_review(storage, review) {
        Ok(expectation) => expectation,
        Err(refusal) => return Refused(refusal),
    };
    let request = StockGrantRequest {
        agent: review.agent.clone(),
        target: StockGrantTarget::Mcp {
            server: review.server.name.clone(),
            operations: review.operations.clone(),
        },
        expires_in_ms: review.expires_in_ms,
    };
    let prepared = match issuance::prepare_grant(
        realm_id,
        principals,
        &request,
        Some(&review.server.connector_approval()),
    ) {
        Ok(prepared) => prepared,
        Err(error) => return Refused(Refusal::InvalidReview(error.to_string())),
    };
    let source = match realm_config_source(storage) {
        Ok(source) => source,
        Err(refusal) => return Refused(refusal),
    };
    let document = source.config_doc_path(&storage.locator.realm);
    let source: Arc<dyn meerkat_core::RealmConfigSource> = Arc::new(source);

    // One administrative custody for every step below.
    let bundle = match issuance::administrative_custody(realms_root, realm_id).await {
        Ok(bundle) => bundle,
        Err(error) => return Refused(Refusal::Unavailable(error.to_string())),
    };
    // The connector clients the realm's effective configuration declares
    // now, not the ones a host installed earlier.
    let effective = match meerkat_core::EffectiveConfigReader::new(Arc::clone(&source))
        .effective_config(&storage.locator.realm)
        .await
    {
        Ok(effective) => effective,
        Err(error) => return Refused(Refusal::Unavailable(error.to_string())),
    };
    let client_profiles =
        match crate::connector_client_auth_profiles(&effective, &storage.locator.realm) {
            Ok(profiles) => profiles,
            Err(_) => return Refused(Refusal::ClientConfigChanged),
        };
    let host_auth = match crate::HostAuthService::from_factory(
        factory,
        bundle.runtime_adapter().provider_auth_runtime_authority(),
    ) {
        Ok(host_auth) => host_auth.with_connector_client_auth_profiles(client_profiles),
        Err(error) => return Refused(Refusal::Unavailable(error.to_string())),
    };

    match host_auth.connector_status(&review.slot).await {
        Ok(status)
            if status.phase == ConnectorAuthPhase::Authorized
                && status.verified_account.as_ref() == Some(&review.account) => {}
        Ok(_) => return Refused(Refusal::AccountChanged),
        Err(error) => return Refused(Refusal::Unavailable(error.to_string())),
    }
    if let Err(refusal) = host_auth
        .connector_check_expectation(&expectation)
        .await
        .map_err(expectation_refusal)
    {
        return Refused(refusal);
    }
    if let Some(binding) = &review.client_profile_binding
        && !host_auth.has_connector_client_auth_binding(binding)
    {
        return Refused(Refusal::ClientConfigChanged);
    }

    // The realm's own document, composed with its ancestors as at start:
    // this write is of the realm's own MCP servers alone, and the realm chain
    // keeps it from changing the `[confinement]` or `[governance]` the realm
    // composes all the same.
    let runtime = ConfigRuntime::new(
        Arc::new(
            FileConfigStore::new(document.clone(), meerkat_models::canonical())
                .with_realm_chain(storage.locator.realm.clone(), source),
        ),
        document.with_file_name(CONFIG_STATE_FILE),
    );
    let generation = match McpRealmPersistTarget::new(&runtime)
        .add(review.server.clone(), Some(review.expected_generation))
        .await
    {
        Ok(snapshot) => snapshot.generation,
        Err(error) => return persist_failure(review, error),
    };

    // The written server's grant is issued and selected on the
    // administrative command's own task, still under this custody: a caller
    // that stops waiting cannot drop the outcome.
    let (realms_root, realm_id) = (realms_root.to_path_buf(), realm_id.to_string());
    let (principals, server) = (principals.clone(), review.server.clone());
    let grant = issuance::on_owned_task(async move {
        #[cfg(test)]
        held_after_write(&document).await;
        let grant = issuance::issue_for_current_installation(
            &bundle,
            &realms_root,
            &realm_id,
            &principals,
            &request,
            prepared,
            &document,
            &server,
            generation,
        )
        .await;
        drop(bundle);
        grant
    })
    .await;
    ReviewedConnectorFinalization::Persisted { generation, grant }
}

/// Test holds right after a finalization's config write, one per config
/// document: that finalization acknowledges and waits for the release
/// before it takes the writer exclusion. Others pass, and holds for other
/// documents are kept.
#[cfg(test)]
type AfterWriteHold = (
    std::path::PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[cfg(test)]
static AFTER_WRITE_HOLDS: std::sync::Mutex<Vec<AfterWriteHold>> = std::sync::Mutex::new(Vec::new());

/// Hold the next finalization that writes `document`.
#[cfg(test)]
fn hold_after_write(
    document: std::path::PathBuf,
    acknowledge: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
) {
    if let Ok(mut holds) = AFTER_WRITE_HOLDS.lock() {
        holds.push((document, acknowledge, release));
    }
}

#[cfg(test)]
async fn held_after_write(document: &std::path::Path) {
    let hold = AFTER_WRITE_HOLDS.lock().ok().and_then(|mut holds| {
        holds
            .iter()
            .position(|(held, _, _)| held == document)
            .map(|index| holds.swap_remove(index))
    });
    if let Some((_, acknowledge, release)) = hold {
        let _ = acknowledge.send(());
        let _ = release.await;
    }
}

/// The realm's config source with the shared realm-to-document projection
/// ([`issuance::realm_config_source`]): the reserved `global` realm's own
/// document is the user-global document, never a shadow under the state
/// root.
fn realm_config_source(
    storage: &ResolvedStorage,
) -> Result<meerkat_store::FilesystemRealmConfigSource, ReviewedConnectorRefusal> {
    issuance::realm_config_source(storage).ok_or_else(|| {
        ReviewedConnectorRefusal::Unavailable(
            "the user-global config document has no location".to_string(),
        )
    })
}

/// A connector owner's answer about the stored credential, as a refusal.
fn expectation_refusal(error: crate::HostAuthError) -> ReviewedConnectorRefusal {
    match error {
        crate::HostAuthError::Connector(
            ConnectorLoginError::ReauthRequired
            | ConnectorLoginError::Verification(ConnectorOAuthRefusal::AccountMismatch),
        ) => ReviewedConnectorRefusal::AccountChanged,
        crate::HostAuthError::Connector(
            error @ (ConnectorLoginError::Slot(_) | ConnectorLoginError::Verification(_)),
        ) => ReviewedConnectorRefusal::CredentialMismatch(error.to_string()),
        other => ReviewedConnectorRefusal::Unavailable(other.to_string()),
    }
}

/// The realm config document's generation sidecar, beside the document.
const CONFIG_STATE_FILE: &str = "config_state.json";

/// The review must name one installation of this realm's connector slot,
/// bound to exactly the reviewed account, and every operation the grant
/// allows must be one the binding declares. Returns what the stored
/// credential must satisfy: the binding's context and the scopes those
/// operations require.
fn check_review(
    storage: &ResolvedStorage,
    review: &ReviewedConnector,
) -> Result<ConnectorCallExpectation, ReviewedConnectorRefusal> {
    let invalid = |reason: &str| ReviewedConnectorRefusal::InvalidReview(reason.to_string());
    if review.slot.realm != storage.locator.realm {
        return Err(invalid("the connector slot belongs to another realm"));
    }
    let Some(binding) = &review.server.connector else {
        return Err(invalid("the reviewed server carries no connector binding"));
    };
    if binding.realm_id != review.slot.realm.as_str()
        || binding.slot_id != review.slot.account.as_str()
        || binding.issuer != review.account.issuer
        || binding.strategy_id != review.account.strategy_id
        || binding.account != review.account.subject
    {
        return Err(invalid(
            "the server's connector binding is not the reviewed slot and account",
        ));
    }
    let mut required_scopes = std::collections::BTreeSet::new();
    match &review.operations {
        McpOperations::All => {
            for operation in binding.operations.values() {
                required_scopes.extend(operation.required_scopes.iter().cloned());
            }
        }
        McpOperations::Exact(operations) => {
            for name in operations {
                let Some(operation) = binding.operations.get(name) else {
                    return Err(invalid(&format!(
                        "operation '{name}' is not declared by the connector binding"
                    )));
                };
                required_scopes.extend(operation.required_scopes.iter().cloned());
            }
        }
    }
    Ok(ConnectorCallExpectation {
        slot: review.slot.clone(),
        account: binding.account.clone(),
        issuer: binding.issuer.clone(),
        client: binding.client.clone(),
        resource: binding.resource.clone(),
        strategy_id: binding.strategy_id.clone(),
        required_scopes,
    })
}

/// A refused write precedes any change; any other failure may follow a
/// started write.
fn persist_failure(
    review: &ReviewedConnector,
    error: McpRealmPersistError,
) -> ReviewedConnectorFinalization {
    use ReviewedConnectorRefusal as Refusal;
    let refusal = match error {
        McpRealmPersistError::ServerExists(name) => Refusal::ServerExists(name),
        McpRealmPersistError::Config(
            meerkat_core::config_runtime::ConfigRuntimeError::GenerationConflict {
                expected,
                current,
            },
        ) => Refusal::ConfigConflict { expected, current },
        McpRealmPersistError::EnvReference(error) => Refusal::InvalidReview(error.to_string()),
        McpRealmPersistError::Access(error) => Refusal::InvalidReview(error.to_string()),
        McpRealmPersistError::Cwd(error) => Refusal::InvalidReview(error.to_string()),
        McpRealmPersistError::ServerNotFound(_) => {
            Refusal::InvalidReview(format!("server '{}' was not added", review.server.name))
        }
        McpRealmPersistError::NotAdmitted(refusal) => Refusal::Unavailable(refusal.to_string()),
        McpRealmPersistError::Config(error) | McpRealmPersistError::OutcomeUnknown(error) => {
            return ReviewedConnectorFinalization::PersistUncertain {
                reason: error.to_string(),
            };
        }
    };
    ReviewedConnectorFinalization::Refused(refusal)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use meerkat_core::connection::{CredentialAccountId, RealmId};
    use meerkat_core::mcp_config::{McpConnectorBinding, McpConnectorOperation, McpServerAccess};
    use meerkat_core::runtime_bootstrap::{RealmLocator, RealmRootChoice};
    use meerkat_core::storage_layout::StorageLayout;
    use meerkat_providers::auth_oauth::{OAuthTokenResult, PkceChallenge};
    use meerkat_providers::auth_store::ProviderAuthPersistence;
    use meerkat_providers::connector_login::{
        ConnectorAccountStrategy, ConnectorOAuthCallback, ConnectorOAuthTarget,
    };
    use meerkat_providers::connector_oauth::{
        AccountSelection, ConnectorAccountObservation, ConnectorOAuthDescriptor,
    };

    use super::super::StockGovernanceError;
    use super::*;

    const STRATEGY: &str = "finalize-fixture-v1";
    const SUBJECT: &str = "subject-1";
    const ACCESS: &str = "access-1";
    const CODE: &str = "code-1";
    const REDIRECT: &str = "http://127.0.0.1:41001/connector/oauth/callback";
    const GRANTED_SCOPE: &str = "files.read";

    fn realm() -> RealmId {
        RealmId::parse("governed").unwrap()
    }

    fn principals() -> StockPrincipals {
        StockPrincipals::for_user(&realm(), 4242).unwrap()
    }

    fn storage_for(root: &Path, realm: RealmId, home: Option<&Path>) -> ResolvedStorage {
        ResolvedStorage {
            layout: StorageLayout::with_injected_roots(
                root.to_path_buf(),
                None,
                home.map(Path::to_path_buf),
                root.to_path_buf(),
            ),
            locator: RealmLocator {
                state_root: root.to_path_buf(),
                realm,
            },
            root_choice: RealmRootChoice::Explicit,
        }
    }

    fn storage(root: &Path) -> ResolvedStorage {
        storage_for(root, realm(), None)
    }

    async fn governed_realm() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        drop(
            super::super::super::open_governed_realm_persistence_with_provider(
                &crate::DiskStorageProvider,
                tmp.path(),
                "governed",
                Some(meerkat_store::RealmBackend::Sqlite),
                None,
                None,
                super::super::super::RealmGovernance::Ungoverned,
            )
            .await
            .expect("an ordinary realm"),
        );
        super::super::govern_stock_realm(tmp.path(), "governed", &principals())
            .await
            .expect("governed");
        tmp
    }

    fn persistence() -> ProviderAuthPersistence {
        ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        )
    }

    /// The factory whose credential persistence the finalizer reads.
    fn factory(root: &Path, persistence: ProviderAuthPersistence) -> crate::AgentFactory {
        crate::AgentFactory::new(root.join("sessions")).with_provider_auth_persistence(persistence)
    }

    fn review() -> ReviewedConnector {
        let slot = CredentialAccountRef {
            realm: realm(),
            account: CredentialAccountId::parse("mail").unwrap(),
        };
        let account = ConnectorVerifiedAccount {
            issuer: "https://issuer.invalid".into(),
            strategy_id: STRATEGY.into(),
            subject: SUBJECT.into(),
        };
        let mut server = McpServerConfig::stdio(
            "mail",
            "/bin/true",
            Vec::new(),
            std::collections::HashMap::new(),
        );
        server.access = Some(McpServerAccess::allowing(["read"]));
        server.connector = Some(McpConnectorBinding {
            realm_id: "governed".into(),
            slot_id: "mail".into(),
            issuer: account.issuer.clone(),
            client: "client".into(),
            resource: "https://api.invalid".into(),
            strategy_id: account.strategy_id.clone(),
            account: account.subject.clone(),
            operations: BTreeMap::from([(
                "read".to_string(),
                McpConnectorOperation {
                    required_scopes: [GRANTED_SCOPE.to_string()].into(),
                    destination_origin: "https://api.invalid".into(),
                    read_only: true,
                },
            )]),
        });
        ReviewedConnector {
            agent: "rkat-agent".into(),
            slot,
            account,
            client_profile_binding: None,
            server,
            expected_generation: 0,
            operations: McpOperations::Exact(["read".to_string()].into()),
            expires_in_ms: Some(3_600_000),
        }
    }

    /// The realm's grant owner bytes, grant selection and config document.
    fn realm_state(root: &Path) -> (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) {
        let paths = meerkat_store::realm_paths_in(root, "governed");
        (
            std::fs::read(meerkat_store::grant_owner_store_path(&paths.root)).unwrap(),
            std::fs::read(super::super::stock_grants_path(&paths)).ok(),
            std::fs::read(&paths.config_path).ok(),
        )
    }

    fn refusal(finalized: &ReviewedConnectorFinalization) -> &ReviewedConnectorRefusal {
        match finalized {
            ReviewedConnectorFinalization::Refused(refusal) => refusal,
            other => panic!("expected a refusal: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_review_that_no_longer_holds_writes_nothing() {
        let tmp = governed_realm().await;
        let before = realm_state(tmp.path());
        let factory = factory(tmp.path(), persistence());

        // The slot holds no credential for the reviewed account.
        let finalized =
            finalize_reviewed_connector(&storage(tmp.path()), &principals(), &factory, &review())
                .await;
        assert!(matches!(
            refusal(&finalized),
            ReviewedConnectorRefusal::AccountChanged
        ));

        // A server bound to another account, to no connector, or a grant
        // for an operation the binding does not declare is not the
        // reviewed installation.
        let mut other_account = review();
        if let Some(binding) = other_account.server.connector.as_mut() {
            binding.account = "subject-2".into();
        }
        let mut unbound = review();
        unbound.server.connector = None;
        let mut undeclared = review();
        undeclared.operations = McpOperations::Exact(["delete".to_string()].into());
        for invalid in [other_account, unbound, undeclared] {
            let finalized = finalize_reviewed_connector(
                &storage(tmp.path()),
                &principals(),
                &factory,
                &invalid,
            )
            .await;
            assert!(matches!(
                refusal(&finalized),
                ReviewedConnectorRefusal::InvalidReview(_)
            ));
        }
        assert_eq!(realm_state(tmp.path()), before, "nothing was written");
    }

    #[tokio::test]
    async fn an_ungoverned_realm_is_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let finalized = finalize_reviewed_connector(
            &storage(tmp.path()),
            &principals(),
            &factory(tmp.path(), persistence()),
            &review(),
        )
        .await;
        assert!(matches!(
            refusal(&finalized),
            ReviewedConnectorRefusal::Unavailable(_)
        ));
    }

    #[test]
    fn the_reserved_global_realm_writes_the_user_global_document() {
        let root = Path::new("/state");
        let home = Path::new("/home/user");
        assert_eq!(
            realm_config_source(&storage_for(root, RealmId::global(), Some(home)))
                .unwrap()
                .config_doc_path(&RealmId::global()),
            home.join(".rkat").join("config.toml")
        );
        assert!(matches!(
            realm_config_source(&storage_for(root, RealmId::global(), None)),
            Err(ReviewedConnectorRefusal::Unavailable(_))
        ));
        assert_eq!(
            realm_config_source(&storage_for(root, realm(), Some(home)))
                .unwrap()
                .config_doc_path(&realm()),
            meerkat_store::realm_paths_in(root, "governed").config_path
        );
    }

    #[test]
    fn a_config_conflict_and_an_existing_server_are_refusals_and_io_is_uncertain() {
        let review = review();
        assert!(matches!(
            persist_failure(
                &review,
                McpRealmPersistError::Config(
                    meerkat_core::config_runtime::ConfigRuntimeError::GenerationConflict {
                        expected: 0,
                        current: 3
                    }
                )
            ),
            ReviewedConnectorFinalization::Refused(ReviewedConnectorRefusal::ConfigConflict {
                expected: 0,
                current: 3
            })
        ));
        assert!(matches!(
            persist_failure(&review, McpRealmPersistError::ServerExists("mail".into())),
            ReviewedConnectorFinalization::Refused(ReviewedConnectorRefusal::ServerExists(_))
        ));
        assert!(matches!(
            persist_failure(
                &review,
                McpRealmPersistError::Config(meerkat_core::config_runtime::ConfigRuntimeError::Io(
                    std::io::Error::other("disk")
                ))
            ),
            ReviewedConnectorFinalization::PersistUncertain { .. }
        ));
    }

    // -----------------------------------------------------------------
    // A real connector credential: a local issuer and a fixture account
    // strategy, signed in through the host's own HostAuthService over the
    // factory's persistence.
    // -----------------------------------------------------------------

    /// The fixture provider: one token, one subject.
    struct FixtureStrategy;

    #[async_trait::async_trait]
    impl ConnectorAccountStrategy for FixtureStrategy {
        fn strategy_id(&self) -> &str {
            STRATEGY
        }

        async fn observe_account(
            &self,
            descriptor: &ConnectorOAuthDescriptor,
            tokens: &OAuthTokenResult,
            _nonce: Option<&str>,
        ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
            if tokens.access_token != ACCESS {
                return Err(ConnectorOAuthRefusal::VerificationUnavailable);
            }
            Ok(ConnectorAccountObservation {
                account: SUBJECT.to_owned(),
                granted_scopes: descriptor.granted_scopes_from_response(tokens),
            })
        }
    }

    /// The PKCE challenge read from the generated authorize URL (the fixture
    /// exercises no authorize endpoint).
    type Challenge = Arc<std::sync::Mutex<Option<String>>>;

    /// A local issuer: discovery and a token endpoint that answers only the
    /// exact authorization-code exchange: `CODE`, the redirect, client and
    /// resource of the review, and the verifier of the challenge taken from
    /// the generated authorize URL.
    async fn issuer() -> (String, Challenge) {
        use axum::http::StatusCode;
        use axum::routing::{get, post};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let metadata = serde_json::json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "code_challenge_methods_supported": ["S256"],
        });
        let challenge = Challenge::default();
        let seen = Arc::clone(&challenge);
        let app = axum::Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(move || {
                    let metadata = metadata.clone();
                    async move { axum::Json(metadata) }
                }),
            )
            .route(
                "/token",
                post(
                    move |axum::Form(form): axum::Form<BTreeMap<String, String>>| {
                        let seen = Arc::clone(&seen);
                        async move {
                            let field = |name: &str| form.get(name).map(String::as_str);
                            let expected = seen.lock().unwrap().clone();
                            let verified = field("code_verifier").is_some_and(|verifier| {
                                expected.as_deref()
                                    == Some(
                                        PkceChallenge::s256_for_verifier(verifier).code.as_str(),
                                    )
                            });
                            if field("grant_type") != Some("authorization_code")
                                || field("code") != Some(CODE)
                                || field("redirect_uri") != Some(REDIRECT)
                                || field("client_id") != Some("client")
                                || field("resource") != Some("https://api.invalid")
                                || !verified
                            {
                                return Err((
                                    StatusCode::BAD_REQUEST,
                                    axum::Json(serde_json::json!({"error": "invalid_grant"})),
                                ));
                            }
                            Ok(axum::Json(serde_json::json!({
                                "access_token": ACCESS,
                                "refresh_token": "refresh-1",
                                "token_type": "Bearer",
                                "expires_in": 3600,
                                "scope": GRANTED_SCOPE,
                            })))
                        }
                    },
                ),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (base, challenge)
    }

    /// A host auth service on the governed realm's own runtime, under its
    /// administrative custody (held by the returned bundle), over the
    /// factory's persistence: the same composition the finalizer uses.
    async fn realm_host_auth(
        root: &Path,
        factory: &crate::AgentFactory,
    ) -> (crate::HostAuthService, crate::PersistenceBundle) {
        let bundle = super::super::issuance::administrative_custody(root, "governed")
            .await
            .expect("the realm's custody");
        let host_auth = crate::HostAuthService::from_factory(
            factory,
            bundle.runtime_adapter().provider_auth_runtime_authority(),
        )
        .expect("the factory's persistence")
        .with_connector_strategy(Arc::new(FixtureStrategy));
        (host_auth, bundle)
    }

    /// Sign the review's slot in through the realm's own host auth service
    /// (released before finalization), and return the verified account.
    /// A completion with a code the issuer did not issue is refused (this
    /// checks only the refusal, not that nothing was stored).
    async fn sign_in(root: &Path, factory: &crate::AgentFactory) -> ConnectorVerifiedAccount {
        let (base, challenge) = issuer().await;
        let (host_auth, bundle) = realm_host_auth(root, factory).await;
        let target = ConnectorOAuthTarget {
            slot: review().slot,
            issuer: base,
            client: "client".into(),
            resource: "https://api.invalid".into(),
            scopes: [GRANTED_SCOPE.to_owned()].into(),
            strategy_id: STRATEGY.into(),
            account: AccountSelection::Discover,
            client_auth_profile: None,
        };
        let start = |host_auth: &crate::HostAuthService| {
            let target = target.clone();
            let challenge = Arc::clone(&challenge);
            let host_auth = host_auth.clone();
            async move {
                let start = host_auth
                    .connector_login_start(&target, REDIRECT)
                    .await
                    .expect("admitted");
                let url = reqwest::Url::parse(&start.authorize_url).unwrap();
                let query: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
                assert_eq!(
                    query.get("code_challenge_method").map(String::as_str),
                    Some("S256")
                );
                *challenge.lock().unwrap() =
                    Some(query.get("code_challenge").expect("a challenge").clone());
                start
            }
        };

        let refused = start(&host_auth).await;
        assert!(
            host_auth
                .connector_login_complete(
                    &target,
                    ConnectorOAuthCallback {
                        redirect_uri: REDIRECT.into(),
                        state: refused.state,
                        code: "code-2".into(),
                        host_attempt_handle: None,
                    },
                )
                .await
                .is_err(),
            "the issuer refuses a code it did not issue"
        );

        let admitted = start(&host_auth).await;
        let account = host_auth
            .connector_login_complete(
                &target,
                ConnectorOAuthCallback {
                    redirect_uri: REDIRECT.into(),
                    state: admitted.state,
                    code: CODE.into(),
                    host_attempt_handle: None,
                },
            )
            .await
            .expect("signed in")
            .verified_account;
        drop(host_auth);
        drop(bundle);
        account
    }

    fn realm_config(root: &Path) -> ConfigRuntime {
        let paths = meerkat_store::realm_paths_in(root, "governed");
        ConfigRuntime::new(
            Arc::new(FileConfigStore::new(
                paths.config_path.clone(),
                meerkat_models::canonical(),
            )),
            paths.config_path.with_file_name(CONFIG_STATE_FILE),
        )
    }

    /// The review of the signed-in account, at the realm's current
    /// configuration generation.
    async fn signed_in_review(
        root: &Path,
        account: &ConnectorVerifiedAccount,
    ) -> ReviewedConnector {
        let mut review = review();
        review.account = account.clone();
        if let Some(binding) = review.server.connector.as_mut() {
            binding.issuer = account.issuer.clone();
            binding.strategy_id = account.strategy_id.clone();
            binding.account = account.subject.clone();
        }
        review.expected_generation = McpRealmPersistTarget::new(&realm_config(root))
            .servers()
            .await
            .expect("realm servers")
            .1;
        review
    }

    #[tokio::test]
    async fn a_current_review_persists_the_server_and_selects_only_its_grant() {
        use super::super::issuance::{StockGrantSlot, StockGrantTarget, grant_stock_agent};
        let tmp = governed_realm().await;
        let factory = factory(tmp.path(), persistence());
        let account = sign_in(tmp.path(), &factory).await;
        assert_eq!(account.subject, SUBJECT);
        let review = signed_in_review(tmp.path(), &account).await;

        // An unrelated grant the agent already holds.
        let catalog = grant_stock_agent(
            tmp.path(),
            "governed",
            &principals(),
            &StockGrantRequest {
                agent: review.agent.clone(),
                target: StockGrantTarget::Catalog,
                expires_in_ms: None,
            },
        )
        .await
        .expect("an unrelated catalog grant")
        .issued;
        let issued_after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let finalized =
            finalize_reviewed_connector(&storage(tmp.path()), &principals(), &factory, &review)
                .await;
        let ReviewedConnectorFinalization::Persisted { generation, grant } = finalized else {
            panic!("finalized: {finalized:?}");
        };
        assert!(generation > review.expected_generation);
        let outcome = grant.expect("issued and selected");

        // The exact server is the realm's own installation now.
        let (servers, current) = McpRealmPersistTarget::new(&realm_config(tmp.path()))
            .servers()
            .await
            .unwrap();
        assert_eq!(current, generation);
        assert_eq!(servers, vec![review.server.clone()]);

        // The server's grant is selected for the agent beside the grant it
        // already held, which is untouched.
        let paths = meerkat_store::realm_paths_in(tmp.path(), "governed");
        let selection = super::super::read_stock_grant_file(&paths)
            .await
            .unwrap()
            .expect("a selection");
        let refs = selection.agents.get("rkat-agent").expect("the agent");
        assert_eq!(
            refs.get(&StockGrantSlot::Mcp("mail".into())),
            Some(&outcome.issued)
        );
        assert_eq!(refs.get(&StockGrantSlot::Catalog), Some(&catalog));
        assert_eq!(refs.get(&StockGrantSlot::Controller), None);
        assert!(outcome.superseded.is_none());

        // The reviewed lifetime is the issued grant's expiry, and the
        // unrelated grant still resolves.
        let owner = super::super::issuance::restore_owner(tmp.path(), "governed").unwrap();
        let executor =
            super::super::issuance::stock_agent_principal(&realm(), "rkat-agent").unwrap();
        let issued = owner
            .resolve_lineage(std::slice::from_ref(&outcome.issued), &executor, None)
            .expect("the issued grant resolves");
        let expires_at_ms = issued.expires_at_ms();
        let lifetime = review.expires_in_ms.unwrap();
        assert!(
            expires_at_ms >= issued_after + lifetime
                && expires_at_ms <= issued_after + lifetime + 60_000,
            "expires at {expires_at_ms}, reviewed {lifetime} ms after {issued_after}"
        );
        assert!(
            owner
                .resolve_lineage(std::slice::from_ref(&catalog), &executor, None)
                .is_ok(),
            "the unrelated grant is preserved"
        );

        // The grant records the reviewed server's connector approval through
        // the shared issuance: it covers the installation bound to the
        // reviewed account, and is refused at use for the same server bound
        // to another account.
        use meerkat_authorization::mcp_installation::{
            McpInstallationRef, check_resolved_installation,
        };
        let installation = |server: &McpServerConfig| {
            McpInstallationRef::new(
                meerkat_authorization_contracts::constraints::ResourceDomain {
                    authority: principals().operator,
                    namespace: "mcp-server/mail".into(),
                },
                "mail",
                &server.connector_approval(),
            )
            .unwrap()
        };
        let mut other_account = review.server.clone();
        if let Some(binding) = other_account.connector.as_mut() {
            binding.account = "subject-2".into();
        }
        // At use: now, inside the issued lifetime (it starts at issuance,
        // after `issued_after`).
        let used_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(
            check_resolved_installation(&issued, &installation(&review.server), used_at).is_ok(),
            "the reviewed approval is covered"
        );
        assert!(
            check_resolved_installation(&issued, &installation(&other_account), used_at).is_err(),
            "another account's binding of the same server is refused at use"
        );
    }

    /// The client profile is re-read from the realm's effective
    /// configuration at finalization: a profile changed after the review
    /// refuses before any write, and the reviewed profile, declared again,
    /// finalizes.
    #[tokio::test]
    async fn the_client_profile_is_read_from_the_realms_effective_config() {
        use meerkat_core::{ConfigStore, ConnectorClientAuthMethod, ConnectorClientConfig};
        let tmp = governed_realm().await;
        let persistence = persistence();
        let factory = factory(tmp.path(), persistence.clone());
        let account = sign_in(tmp.path(), &factory).await;
        let paths = meerkat_store::realm_paths_in(tmp.path(), "governed");
        let store: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(
            paths.config_path.clone(),
            meerkat_models::canonical(),
        ));
        let declare = |authorize_params: Vec<(String, String)>| {
            let store = Arc::clone(&store);
            let persistence = persistence.clone();
            async move {
                crate::put_connector_client(
                    Arc::clone(&store),
                    &persistence,
                    &realm(),
                    "mail-client",
                    ConnectorClientConfig {
                        issuer: "https://issuer.invalid".into(),
                        client: "client".into(),
                        client_auth: ConnectorClientAuthMethod::None,
                        authorize_params,
                        resource_indicator: true,
                        protocol: None,
                        registered_redirect_uris: Vec::new(),
                    },
                    crate::ConnectorClientSecretUpdate::Keep,
                )
                .await
                .expect("declared");
                crate::connector_client_auth_profiles(&store.get().await.unwrap(), &realm())
                    .unwrap()
                    .into_iter()
                    .find(|profile| profile.id == "mail-client")
                    .expect("the profile")
                    .binding()
            }
        };
        let reviewed_binding = declare(Vec::new()).await;
        let mut review = signed_in_review(tmp.path(), &account).await;
        review.client_profile_binding = Some(reviewed_binding.clone());

        // The realm's profile changes after the review.
        let changed = declare(vec![("prompt".into(), "consent".into())]).await;
        assert_ne!(changed, reviewed_binding);
        let before = realm_state(tmp.path());
        let finalized =
            finalize_reviewed_connector(&storage(tmp.path()), &principals(), &factory, &review)
                .await;
        assert!(
            matches!(
                refusal(&finalized),
                ReviewedConnectorRefusal::ClientConfigChanged
            ),
            "{finalized:?}"
        );
        assert_eq!(realm_state(tmp.path()), before, "nothing was written");

        // Declared as reviewed again, at the realm's current generation.
        assert_eq!(declare(Vec::new()).await, reviewed_binding);
        let mut review = signed_in_review(tmp.path(), &account).await;
        review.client_profile_binding = Some(reviewed_binding);
        let finalized =
            finalize_reviewed_connector(&storage(tmp.path()), &principals(), &factory, &review)
                .await;
        let ReviewedConnectorFinalization::Persisted { grant, .. } = finalized else {
            panic!("finalized: {finalized:?}");
        };
        let issued = grant.expect("issued and selected").issued;
        let owner = super::super::issuance::restore_owner(tmp.path(), "governed").unwrap();
        let executor =
            super::super::issuance::stock_agent_principal(&realm(), "rkat-agent").unwrap();
        assert!(
            owner
                .resolve_lineage(std::slice::from_ref(&issued), &executor, None)
                .is_ok(),
            "the issued grant resolves"
        );
    }

    /// The written server is replaced after the config write and before the
    /// selection: nothing is issued or selected, and the existing selection
    /// is unchanged.
    #[tokio::test]
    async fn a_server_replaced_after_the_write_is_not_selected() {
        use super::super::issuance::{StockGrantTarget, grant_stock_agent};
        let tmp = governed_realm().await;
        let factory = factory(tmp.path(), persistence());
        let account = sign_in(tmp.path(), &factory).await;
        let review = signed_in_review(tmp.path(), &account).await;
        grant_stock_agent(
            tmp.path(),
            "governed",
            &principals(),
            &StockGrantRequest {
                agent: review.agent.clone(),
                target: StockGrantTarget::Catalog,
                expires_in_ms: None,
            },
        )
        .await
        .expect("an existing selection");
        let paths = meerkat_store::realm_paths_in(tmp.path(), "governed");
        let selection_before = std::fs::read(super::super::stock_grants_path(&paths)).unwrap();

        let (acknowledge, acknowledged) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        hold_after_write(paths.config_path.clone(), acknowledge, released);
        let replace = async {
            acknowledged
                .await
                .expect("the finalization wrote the server");
            // Another config writer replaces the written server with one bound
            // to another account.
            let target_config = realm_config(tmp.path());
            let target = McpRealmPersistTarget::new(&target_config);
            target.remove("mail", None).await.expect("removed");
            let mut other = review.server.clone();
            if let Some(binding) = other.connector.as_mut() {
                binding.account = "subject-2".into();
            }
            target.add(other, None).await.expect("replaced");
            release.send(()).unwrap();
        };
        let (storage, principals) = (storage(tmp.path()), principals());
        let (finalized, ()) = tokio::join!(
            finalize_reviewed_connector(&storage, &principals, &factory, &review),
            replace
        );
        let ReviewedConnectorFinalization::Persisted { grant, .. } = finalized else {
            panic!("finalized: {finalized:?}");
        };
        assert!(
            matches!(
                grant,
                Err(StockGrantError::Refused(
                    StockGovernanceError::InstallationChanged(_)
                ))
            ),
            "{grant:?}"
        );
        assert_eq!(
            std::fs::read(super::super::stock_grants_path(&paths)).unwrap(),
            selection_before,
            "the existing selection is unchanged"
        );
    }

    /// The caller stops waiting after the config write: the grant leg still
    /// completes on the command's own task and its selection lands.
    #[tokio::test]
    async fn a_caller_lost_after_the_write_keeps_the_grant_outcome() {
        use super::super::issuance::StockGrantSlot;
        let tmp = governed_realm().await;
        let factory = factory(tmp.path(), persistence());
        let account = sign_in(tmp.path(), &factory).await;
        let review = signed_in_review(tmp.path(), &account).await;
        let paths = meerkat_store::realm_paths_in(tmp.path(), "governed");

        let (acknowledge, acknowledged) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        hold_after_write(paths.config_path.clone(), acknowledge, released);
        let root = tmp.path().to_path_buf();
        let caller = tokio::spawn(async move {
            finalize_reviewed_connector(&storage(&root), &principals(), &factory, &review).await
        });
        acknowledged
            .await
            .expect("the finalization wrote the server");
        caller.abort();
        assert!(
            caller.await.unwrap_err().is_cancelled(),
            "the caller is cancelled and joined"
        );
        release.send(()).unwrap();

        // The owned issuance renames the selection into place and then still
        // holds the exclusively opened grant owner store while it syncs the
        // directory and finishes, so a visible selection is not yet a
        // restorable owner. The wait covers both before the selected lineage
        // is resolved.
        let mut restored = Err(None);
        for _ in 0..500 {
            let selected = super::super::read_stock_grant_file(&paths)
                .await
                .ok()
                .flatten()
                .and_then(|file| {
                    file.agents
                        .get("rkat-agent")
                        .and_then(|refs| refs.get(&StockGrantSlot::Mcp("mail".into())).cloned())
                });
            restored = match selected {
                Some(selected) => super::super::issuance::restore_owner(tmp.path(), "governed")
                    .map(|owner| (selected, owner))
                    .map_err(Some),
                None => Err(None),
            };
            if restored.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let (selected, owner) =
            restored.expect("the owned task selected the grant and released the owner store");
        let executor =
            super::super::issuance::stock_agent_principal(&realm(), "rkat-agent").unwrap();
        assert!(
            owner
                .resolve_lineage(std::slice::from_ref(&selected), &executor, None)
                .is_ok(),
            "the selected grant resolves"
        );
    }

    #[tokio::test]
    async fn a_sign_out_after_the_review_refuses_before_any_write() {
        let tmp = governed_realm().await;
        let factory = factory(tmp.path(), persistence());
        let account = sign_in(tmp.path(), &factory).await;
        let review = signed_in_review(tmp.path(), &account).await;

        // The live owner state changes after the review was built: the slot
        // is signed out through the realm's own host auth service.
        let (host_auth, bundle) = realm_host_auth(tmp.path(), &factory).await;
        host_auth
            .connector_logout(&review.slot)
            .await
            .expect("signed out");
        drop(host_auth);
        drop(bundle);
        let before = realm_state(tmp.path());

        let finalized =
            finalize_reviewed_connector(&storage(tmp.path()), &principals(), &factory, &review)
                .await;
        assert!(
            matches!(
                refusal(&finalized),
                ReviewedConnectorRefusal::AccountChanged
            ),
            "{finalized:?}"
        );
        assert_eq!(realm_state(tmp.path()), before, "nothing was written");
    }

    #[tokio::test]
    async fn drift_after_the_review_refuses_before_any_write() {
        let tmp = governed_realm().await;
        let factory = factory(tmp.path(), persistence());
        let account = sign_in(tmp.path(), &factory).await;
        let current = signed_in_review(tmp.path(), &account).await;
        let before = realm_state(tmp.path());

        // Another account than the slot now holds.
        let mut other_account = current.clone();
        other_account.account.subject = "subject-2".into();
        if let Some(binding) = other_account.server.connector.as_mut() {
            binding.account = "subject-2".into();
        }
        // A configuration generation the realm has moved past.
        let mut stale = current.clone();
        stale.expected_generation += 7;
        // A client profile the host no longer installs.
        let mut client = current.clone();
        client.client_profile_binding = Some("removed@0000".into());
        // The credential was granted for another resource, or lacks a scope
        // an allowed operation requires.
        let mut resource = current.clone();
        if let Some(binding) = resource.server.connector.as_mut() {
            binding.resource = "https://other.invalid".into();
        }
        let mut scope = current.clone();
        if let Some(binding) = scope.server.connector.as_mut()
            && let Some(read) = binding.operations.get_mut("read")
        {
            read.required_scopes.insert("files.write".into());
        }

        for (review, expected) in [
            (other_account, "account"),
            (stale, "conflict"),
            (client, "client"),
            (resource, "credential"),
            (scope, "credential"),
        ] {
            let finalized =
                finalize_reviewed_connector(&storage(tmp.path()), &principals(), &factory, &review)
                    .await;
            let matched = match refusal(&finalized) {
                ReviewedConnectorRefusal::AccountChanged => expected == "account",
                ReviewedConnectorRefusal::ConfigConflict { .. } => expected == "conflict",
                ReviewedConnectorRefusal::ClientConfigChanged => expected == "client",
                ReviewedConnectorRefusal::CredentialMismatch(_) => expected == "credential",
                _ => false,
            };
            assert!(matched, "{expected}: {finalized:?}");
        }
        assert_eq!(realm_state(tmp.path()), before, "nothing was written");
    }
}
