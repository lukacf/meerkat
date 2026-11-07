//! Realm custody of MCP stdio servers' service secrets in the facade.
//!
//! Two halves share one reference
//! ([`meerkat_core::auth::McpServiceSecretRef`]): the realm whose own
//! configuration document defines the server, the server, the environment
//! slot and the server's canonical launch meaning.
//!
//! * [`RealmMcpServiceSecrets`] resolves a launch: the realm whose OWN
//!   document defines the server in the host realm's effective
//!   configuration (the nearest definition on the composed chain, by the
//!   configuration owner's own chain authority), that the launched
//!   definition is exactly that realm's current one, and then only that
//!   realm's row for each declared slot. Every runtime-backed host installs
//!   it over its MCP credential source ([`RealmMcpServiceSecrets::serve_for`]).
//! * The defining realm's owner sets and removes values for the exact
//!   definition it reviewed ([`set_mcp_service_secret`],
//!   [`remove_mcp_service_secret`]), revalidated under the realm config
//!   document's writer exclusion and the credential mutation owner.
//!
//! Neither half authorizes a launch: the existing installation, attachment,
//! account and access checks decide that. No value is ever read from the
//! host environment, and none appears in configuration, `Debug`, errors or
//! records.
//!
//! Coverage limit of this first implementation: only servers defined in a
//! realm's own configuration document have custody. A server from an
//! `mcp.toml` file, a host-supplied configuration or a live `mcp/add` that
//! declares `secret_env` is refused locally with
//! [`McpServiceSecretRefusal::NoOwner`] (or `NoCustody` where the host's
//! credential source serves none).

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use meerkat_core::McpServerConfig;
use meerkat_core::auth::{
    McpServiceSecretPresence, McpServiceSecretRef, McpServiceSecretRefusal, McpServiceSecretValues,
    TokenStore, load_mcp_service_secret, observe_mcp_service_secret,
};
use meerkat_core::connection::RealmId;
use meerkat_core::mcp_config::McpTransportConfig;

/// Finds the realm whose own document defines a server name, with that
/// definition.
type DefinitionOwnerLookup = Arc<
    dyn Fn(String) -> BoxFuture<'static, Result<Option<(RealmId, McpServerConfig)>, String>>
        + Send
        + Sync,
>;

/// The service-secret custody of the realms whose own configuration defines
/// a host's MCP stdio servers (see the module docs).
///
/// It never reads the nearest stored row, never falls back from a child
/// realm's identical definition to its parent's row, and authorizes nothing.
#[derive(Clone)]
pub struct RealmMcpServiceSecrets {
    owner_of: DefinitionOwnerLookup,
    token_store: Arc<dyn TokenStore>,
}

/// What a launch of a server would find, without reading any value: the
/// realm whose own configuration defines it and, per declared slot, whether
/// a value is stored for its current definition. A stored value does not
/// say which write stored it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServiceSecretObservation {
    pub owner: RealmId,
    pub slots: BTreeMap<String, McpServiceSecretPresence>,
}

impl RealmMcpServiceSecrets {
    /// Custody for servers in `head`'s effective configuration, composed
    /// from `source`'s realm documents exactly as
    /// [`meerkat_core::EffectiveConfigReader`] composes it, with values in
    /// `token_store` (the realms' credential store).
    pub fn from_realm_source(
        source: Arc<dyn meerkat_core::RealmConfigSource>,
        head: RealmId,
        token_store: Arc<dyn TokenStore>,
    ) -> Self {
        let owner_of: DefinitionOwnerLookup = Arc::new(move |name: String| {
            let reader = meerkat_core::EffectiveConfigReader::new(Arc::clone(&source));
            let head = head.clone();
            Box::pin(async move {
                reader
                    .mcp_server_owner(&head, &name)
                    .await
                    .map_err(|error| error.to_string())
            })
        });
        Self {
            owner_of,
            token_store,
        }
    }

    /// Custody for servers in a builder's effective configuration: its head
    /// document and realm-inheritance slot, read at each launch.
    #[cfg(feature = "mcp")]
    pub(crate) fn from_effective_config_view(
        view: crate::service_factory::EffectiveConfigView,
        token_store: Arc<dyn TokenStore>,
    ) -> Self {
        let owner_of: DefinitionOwnerLookup = Arc::new(move |name: String| {
            let view = view.clone();
            Box::pin(async move {
                view.mcp_server_owner(&name)
                    .await
                    .map_err(|error| error.to_string())
            })
        });
        Self {
            owner_of,
            token_store,
        }
    }

    /// The realm defining `server` and its current definition there, which
    /// must launch exactly as `server` does (for an HTTP server, equal
    /// whole definitions).
    async fn current_owner(
        &self,
        server: &McpServerConfig,
    ) -> Result<(RealmId, McpServerConfig), McpServiceSecretRefusal> {
        let (owner, definition) = (self.owner_of)(server.name.clone())
            .await
            .map_err(|reason| McpServiceSecretRefusal::Unavailable { reason })?
            .ok_or(McpServiceSecretRefusal::NoOwner)?;
        let current = match (
            definition.stdio_launch_meaning(),
            server.stdio_launch_meaning(),
        ) {
            (Some(current), Some(launched)) => current == launched,
            (None, None) => definition == *server,
            _ => false,
        };
        if current {
            Ok((owner, definition))
        } else {
            Err(McpServiceSecretRefusal::DefinitionChanged)
        }
    }

    /// The defining realm of `server` and the reference of each slot its
    /// current definition declares.
    async fn references(
        &self,
        server: &McpServerConfig,
    ) -> Result<(RealmId, Vec<McpServiceSecretRef>), McpServiceSecretRefusal> {
        let (owner, definition) = self.current_owner(server).await?;
        let declared = match &definition.transport {
            McpTransportConfig::Stdio(stdio) => stdio.secret_env.iter().collect(),
            McpTransportConfig::Http(_) => Vec::new(),
        };
        let references = declared
            .into_iter()
            .map(|env| McpServiceSecretRef::for_definition(owner.clone(), &definition, env))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| McpServiceSecretRefusal::Unavailable {
                reason: error.to_string(),
            })?;
        Ok((owner, references))
    }

    /// The values `server`'s declared slots launch with: an unattended read
    /// of the defining realm's exact rows. A missing value refuses the
    /// launch; it is never taken from anywhere else. A server that declares
    /// no secrets has none, and nothing is read for it.
    pub async fn resolve(
        &self,
        server: &McpServerConfig,
    ) -> Result<McpServiceSecretValues, McpServiceSecretRefusal> {
        if !matches!(&server.transport, McpTransportConfig::Stdio(stdio) if !stdio.secret_env.is_empty())
        {
            return Ok(McpServiceSecretValues::new());
        }
        let (_, references) = self.references(server).await?;
        let mut values = McpServiceSecretValues::new();
        for reference in references {
            match load_mcp_service_secret(self.token_store.as_ref(), &reference).await {
                Ok(Some(value)) => values.insert(reference.env(), value),
                Ok(None) => {
                    return Err(McpServiceSecretRefusal::NotStored {
                        env: reference.env().to_owned(),
                    });
                }
                Err(error) => {
                    return Err(McpServiceSecretRefusal::Unavailable {
                        reason: error.to_string(),
                    });
                }
            }
        }
        Ok(values)
    }

    /// What a launch of `server` would find, by the same resolution as
    /// [`Self::resolve`], observed as a status read that reads no value.
    pub async fn observe(
        &self,
        server: &McpServerConfig,
    ) -> Result<McpServiceSecretObservation, McpServiceSecretRefusal> {
        let (owner, references) = self.references(server).await?;
        let mut slots = BTreeMap::new();
        for reference in references {
            let presence = observe_mcp_service_secret(self.token_store.as_ref(), &reference)
                .await
                .map_err(|error| McpServiceSecretRefusal::Unavailable {
                    reason: error.to_string(),
                })?;
            slots.insert(reference.env().to_owned(), presence);
        }
        Ok(McpServiceSecretObservation { owner, slots })
    }

    /// `inner` serving this custody's service secrets. Every other method
    /// (stored and interactive OAuth credentials, the credential realm, the
    /// HTTP egress profile and the selected account) is `inner`'s own, and a
    /// service-secret answer from `inner` other than
    /// [`McpServiceSecretRefusal::NoCustody`] stands, so an explicit
    /// resolver's own custody is never replaced.
    #[cfg(feature = "mcp")]
    pub fn serve_for(
        self,
        inner: Arc<dyn meerkat_mcp::McpAuthResolver>,
    ) -> Arc<dyn meerkat_mcp::McpAuthResolver> {
        Arc::new(ServiceSecretCustodyResolver {
            inner,
            custody: self,
        })
    }
}

/// A credential source with realm service-secret custody added; see
/// [`RealmMcpServiceSecrets::serve_for`].
#[cfg(feature = "mcp")]
struct ServiceSecretCustodyResolver {
    inner: Arc<dyn meerkat_mcp::McpAuthResolver>,
    custody: RealmMcpServiceSecrets,
}

#[cfg(feature = "mcp")]
#[async_trait::async_trait]
impl meerkat_mcp::McpAuthResolver for ServiceSecretCustodyResolver {
    async fn stored_bearer_token(
        &self,
        target: &meerkat_providers::mcp_oauth::McpServerIdentity,
    ) -> Result<Option<String>, meerkat_providers::mcp_oauth::McpOAuthError> {
        self.inner.stored_bearer_token(target).await
    }

    async fn interactive_login(
        &self,
        target: &meerkat_providers::mcp_oauth::McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<String, meerkat_providers::mcp_oauth::McpOAuthError> {
        self.inner.interactive_login(target, www_authenticate).await
    }

    fn credential_realm(&self) -> Option<RealmId> {
        self.inner.credential_realm()
    }

    fn http_egress(&self) -> Option<meerkat_mcp::HttpEgressProfile> {
        self.inner.http_egress()
    }

    async fn stdio_service_secrets(
        &self,
        server: &McpServerConfig,
    ) -> Result<McpServiceSecretValues, McpServiceSecretRefusal> {
        match self.inner.stdio_service_secrets(server).await {
            Err(McpServiceSecretRefusal::NoCustody) => self.custody.resolve(server).await,
            answered => answered,
        }
    }

    fn selected_account(
        &self,
        target: &meerkat_providers::mcp_oauth::McpServerIdentity,
    ) -> meerkat_core::OperationAccount {
        self.inner.selected_account(target)
    }
}

#[cfg(all(feature = "session-store", feature = "local-authorization"))]
pub use update::{
    McpServiceSecretRemoval, McpServiceSecretUpdateError, remove_mcp_service_secret,
    remove_mcp_service_secret_entered, set_mcp_service_secret, set_mcp_service_secret_entered,
};

/// The defining realm owner's exact-reference mutations.
#[cfg(all(feature = "session-store", feature = "local-authorization"))]
mod update {
    use std::sync::Arc;

    use meerkat_core::auth::{
        CredentialMutationError, CredentialMutationFn, CredentialMutationOutcome,
        McpServiceSecretRef, McpServiceSecretValue, TokenKey, TokenStore, TokenStoreAccess,
        TokenStoreError, clear_mcp_service_secret, save_mcp_service_secret,
    };
    use meerkat_core::storage_layout::ResolvedStorage;
    use meerkat_core::{
        AdministeredChange, AdministrationEntry, AdministrationEntryRefusal, AdministrationOutcome,
        EnteredAdministration, McpServerConfig,
    };

    /// Which value [`remove_mcp_service_secret`] removes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum McpServiceSecretRemoval {
        /// A deliberate removal from the current installation: the realm's
        /// own document must still list exactly the reviewed definition. Its
        /// launches are refused until a value is set again.
        Current,
        /// Cleanup after the reviewed definition was replaced or deleted:
        /// refused while the realm's own document still lists a definition
        /// that launches as the reviewed one did (the same reference), so a
        /// delayed cleanup never removes the value of a current replacement
        /// or identical reinstallation.
        Obsolete,
    }

    /// Why a service-secret value was not set or removed, or how far the
    /// attempt got. No variant carries the value.
    #[derive(Debug, thiserror::Error)]
    pub enum McpServiceSecretUpdateError {
        /// The reviewed definition or slot cannot hold a service secret, or
        /// the value is empty. Nothing was read or written.
        #[error("invalid MCP service secret update: {0}")]
        Invalid(String),
        /// The realm's own document no longer lists exactly the reviewed
        /// definition. Nothing was written.
        #[error("MCP server '{0}' is no longer installed in the realm exactly as reviewed")]
        DefinitionChanged(String),
        /// An obsolete-value cleanup found the definition current again.
        /// Nothing was removed.
        #[error("MCP server '{0}' is installed in the realm with the reviewed definition")]
        StillCurrent(String),
        /// The realm's config document could not be held or read (a config
        /// write or administrative read is in progress, or it is
        /// unreadable). Nothing was written.
        #[error("the realm's configuration is unavailable: {0}")]
        ConfigurationUnavailable(String),
        /// The credential mutation owner did not admit the update. Nothing
        /// was written.
        #[error("MCP service secret update ownership is unavailable: {0}")]
        Ownership(String),
        /// The host's entry check refused the caller at the update's entry
        /// ([`set_mcp_service_secret_entered`]). Nothing was written.
        #[error(transparent)]
        NotAdmitted(AdministrationEntryRefusal),
        /// The write began and its result is not known: the slot may hold
        /// the previous value, the new one or none. Presence does not settle
        /// it; repeating the same update does.
        #[error("MCP service secret update outcome is unknown: {0}")]
        OutcomeUnknown(String),
    }

    /// The realm's own config document, by the projection the reviewed
    /// installation workflow shares.
    fn realm_document(
        storage: &ResolvedStorage,
    ) -> Result<std::path::PathBuf, McpServiceSecretUpdateError> {
        crate::governed_realm::stock::issuance::realm_config_source(storage)
            .map(|source| source.config_doc_path(&storage.locator.realm))
            .ok_or_else(|| {
                McpServiceSecretUpdateError::ConfigurationUnavailable(
                    "the realm's own config document has no location".to_owned(),
                )
            })
    }

    /// The reference of `env` of `reviewed` in `storage`'s realm.
    fn reference(
        storage: &ResolvedStorage,
        reviewed: &McpServerConfig,
        env: &str,
    ) -> Result<McpServiceSecretRef, McpServiceSecretUpdateError> {
        McpServiceSecretRef::for_definition(storage.locator.realm.clone(), reviewed, env)
            .map_err(|error| McpServiceSecretUpdateError::Invalid(error.to_string()))
    }

    /// What the update does once the realm's installation was checked.
    enum Write {
        Set(McpServiceSecretValue),
        Clear,
    }

    /// The admission of the entry-free forms: every update is admitted,
    /// holding nothing.
    struct AdmitAll;

    impl AdministrationEntry for AdmitAll {
        fn enter(&self) -> Result<EnteredAdministration, AdministrationEntryRefusal> {
            Ok(EnteredAdministration::admitted())
        }
    }

    /// The admission `entry` granted at an update's entry, held by the update
    /// until the credential mutation owner has received its durable result.
    type EnteredSlot = Arc<std::sync::Mutex<Option<EnteredAdministration>>>;

    /// Why an update wrote nothing, or nothing known.
    enum NotWritten {
        /// Refused before any write: nothing was written.
        Refused(McpServiceSecretUpdateError),
        /// The store write began and failed: its result is not known.
        Store(TokenStoreError),
    }

    /// Run `write` for `reference` under the credential mutation owner of
    /// that exact reference (every update of it, in this or another process
    /// sharing the coordinator, runs one at a time), and inside it under the
    /// realm config document's writer exclusion, held through the write: no
    /// config writer or administrative command changes the installation
    /// between the check `admits` makes and the write. `entry` admits the
    /// caller after both custodies are held and the installation checked,
    /// immediately before the write, and its admission is held until the
    /// mutation owner has received the write's durable result. The mutation
    /// owner receives the write's own durable result: the persisted row, the
    /// removal, or the error that left nothing (or nothing known) written.
    async fn update(
        persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
        document: std::path::PathBuf,
        reference: McpServiceSecretRef,
        admits: impl FnOnce(Option<&McpServerConfig>) -> Result<(), McpServiceSecretUpdateError>
        + Send
        + 'static,
        write: Write,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<(), McpServiceSecretUpdateError> {
        let token_store = persistence.token_store();
        let outcome = Arc::new(std::sync::Mutex::new(None));
        let slot = Arc::clone(&outcome);
        let entered: EnteredSlot = Arc::new(std::sync::Mutex::new(None));
        let entered_slot = Arc::clone(&entered);
        let key = TokenKey::mcp_service_secret(&reference);
        let mutation: CredentialMutationFn = Box::new(move || {
            Box::pin(async move {
                let written = write_if_admitted(
                    token_store.as_ref(),
                    &document,
                    &reference,
                    admits,
                    entry.as_ref(),
                    &entered_slot,
                    write,
                )
                .await;
                let (result, durable) = match written {
                    Ok(durable) => (Ok(()), Ok(durable)),
                    Err(NotWritten::Refused(refusal)) => {
                        let error = CredentialMutationError::Operation(refusal.to_string());
                        (Err(refusal), Err(error))
                    }
                    Err(NotWritten::Store(error)) => (
                        Err(McpServiceSecretUpdateError::OutcomeUnknown(
                            error.to_string(),
                        )),
                        Err(CredentialMutationError::from_token_store(error)),
                    ),
                };
                *slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
                durable
            })
        });
        let admitted = persistence
            .refresh_coordinator()
            .with_exclusive_mutation_for(key, TokenStoreAccess::Interactive, mutation)
            .await;
        let result = outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        // The update has settled: only now is the entered admission released.
        drop(
            entered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
        match (admitted, result) {
            // The update ran to its own typed result.
            (_, Some(result)) => result,
            // Ownership was never acquired, or admission was refused while
            // earlier work for the reference still runs: nothing started.
            (Err(CredentialMutationError::LockFailed(error)), None) => {
                Err(McpServiceSecretUpdateError::Ownership(error))
            }
            (Err(error @ CredentialMutationError::CredentialAccess { .. }), None) => {
                Err(McpServiceSecretUpdateError::Ownership(error.to_string()))
            }
            // Anything else (including cancellation) lost the outcome.
            (Err(error), None) => Err(McpServiceSecretUpdateError::OutcomeUnknown(
                error.to_string(),
            )),
            (Ok(_), None) => Err(McpServiceSecretUpdateError::OutcomeUnknown(
                "the coordinator returned without the update's result".to_owned(),
            )),
        }
    }

    /// Take the realm config document's writer exclusion, let `admits` check
    /// the installation it holds, let `entry` admit the caller, then write,
    /// still under the exclusion. The entered admission goes to `entered`.
    async fn write_if_admitted(
        token_store: &dyn TokenStore,
        document: &std::path::Path,
        reference: &McpServiceSecretRef,
        admits: impl FnOnce(Option<&McpServerConfig>) -> Result<(), McpServiceSecretUpdateError>,
        entry: &dyn AdministrationEntry,
        entered: &EnteredSlot,
        write: Write,
    ) -> Result<CredentialMutationOutcome, NotWritten> {
        let (_exclusion, installed, _) =
            crate::governed_realm::installation_under_writer_exclusion_in(
                document,
                reference.server(),
            )
            .map_err(|refusal| {
                NotWritten::Refused(McpServiceSecretUpdateError::ConfigurationUnavailable(
                    refusal.to_string(),
                ))
            })?;
        admits(installed.as_ref()).map_err(NotWritten::Refused)?;
        let change = administered_change(reference, &write);
        let admission = entry
            .enter()
            .map_err(|refusal| {
                NotWritten::Refused(McpServiceSecretUpdateError::NotAdmitted(refusal))
            })?
            .for_change(change);
        *entered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(admission);
        let written = match write {
            Write::Set(value) => save_mcp_service_secret(token_store, reference, value.expose())
                .await
                .map(CredentialMutationOutcome::Persisted),
            Write::Clear => clear_mcp_service_secret(token_store, reference)
                .await
                .map(|()| CredentialMutationOutcome::Cleared),
        }
        .map_err(NotWritten::Store);
        // The write's durable result is known: the admission's record says
        // how it ended, whoever releases the admission last.
        if let Some(admission) = entered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            admission.settle(if written.is_ok() {
                AdministrationOutcome::Committed
            } else {
                AdministrationOutcome::Unknown
            });
        }
        written
    }

    /// What an update of `reference` changes, as its admission's record
    /// names it: the exact slot, never the value.
    fn administered_change(reference: &McpServiceSecretRef, write: &Write) -> AdministeredChange {
        let (realm, server, env, meaning) = (
            reference.realm().clone(),
            reference.server().to_owned(),
            reference.env().to_owned(),
            reference.meaning().to_owned(),
        );
        match write {
            Write::Set(_) => AdministeredChange::McpServiceSecretSet {
                realm,
                server,
                env,
                meaning,
            },
            Write::Clear => AdministeredChange::McpServiceSecretRemove {
                realm,
                server,
                env,
                meaning,
            },
        }
    }

    /// Store `value` for slot `env` of `reviewed`, the MCP server
    /// installation `storage`'s realm defines in its own config document, as
    /// the realm's owner reviewed it.
    ///
    /// The value is bound to the reviewed definition's exact reference, and
    /// stored only while the realm's own document lists exactly `reviewed`
    /// (its whole typed definition), checked under the document's writer
    /// exclusion held through the write. A concurrent replacement under the
    /// same name is a different reference and never receives the value; a
    /// changed installation refuses with nothing written. Setting again
    /// replaces the value; a launch that already read the earlier one keeps
    /// it. The caller is the realm's authenticated configuration owner: this
    /// host-side function is not an authentication.
    pub async fn set_mcp_service_secret(
        storage: &ResolvedStorage,
        persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
        reviewed: &McpServerConfig,
        env: &str,
        value: &str,
    ) -> Result<McpServiceSecretRef, McpServiceSecretUpdateError> {
        set_mcp_service_secret_entered(
            storage,
            persistence,
            reviewed,
            env,
            McpServiceSecretValue::new(value.to_owned()),
            Arc::new(AdmitAll),
        )
        .await
    }

    /// [`set_mcp_service_secret`] for an authenticated caller whose host
    /// re-checks it at the update's entry: `entry` runs inside the credential
    /// mutation owner's exclusion for the exact reference, after the realm
    /// config document's writer exclusion is taken and the installation
    /// re-checked as exactly `reviewed`, immediately before the value is
    /// stored. A refusal there stores nothing
    /// ([`McpServiceSecretUpdateError::NotAdmitted`]). The entered admission
    /// is held until the mutation owner has received the write's durable
    /// result and this update has settled; an entered write that fails is
    /// [`McpServiceSecretUpdateError::OutcomeUnknown`], never a success. An
    /// admission that states its administrator
    /// ([`EnteredAdministration::administered_by`]) hands the host's
    /// recorder one record when the update settles, even after the caller
    /// stopped waiting: the exact slot set, committed or unknown.
    pub async fn set_mcp_service_secret_entered(
        storage: &ResolvedStorage,
        persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
        reviewed: &McpServerConfig,
        env: &str,
        value: McpServiceSecretValue,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<McpServiceSecretRef, McpServiceSecretUpdateError> {
        let reference = reference(storage, reviewed, env)?;
        if value.expose().is_empty() {
            return Err(McpServiceSecretUpdateError::Invalid(
                "a service secret value cannot be empty".to_owned(),
            ));
        }
        let document = realm_document(storage)?;
        let reviewed_owned = reviewed.clone();
        update(
            persistence,
            document,
            reference.clone(),
            move |installed| {
                if installed == Some(&reviewed_owned) {
                    Ok(())
                } else {
                    Err(McpServiceSecretUpdateError::DefinitionChanged(
                        reviewed_owned.name,
                    ))
                }
            },
            Write::Set(value),
            entry,
        )
        .await?;
        Ok(reference)
    }

    /// Remove the value of slot `env` of `reviewed` in `storage`'s realm:
    /// from the current installation, or as cleanup of a replaced or deleted
    /// one ([`McpServiceSecretRemoval`]). Removing a value that is not
    /// stored succeeds. Checked under the realm config document's writer
    /// exclusion held through the removal; a launch that already read the
    /// value keeps it, and immediate cutoff is the existing admission and
    /// connection-close owners' work.
    pub async fn remove_mcp_service_secret(
        storage: &ResolvedStorage,
        persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
        reviewed: &McpServerConfig,
        env: &str,
        removal: McpServiceSecretRemoval,
    ) -> Result<(), McpServiceSecretUpdateError> {
        remove_mcp_service_secret_entered(
            storage,
            persistence,
            reviewed,
            env,
            removal,
            Arc::new(AdmitAll),
        )
        .await
    }

    /// [`remove_mcp_service_secret`] for an authenticated caller whose host
    /// re-checks it at the removal's entry, exactly as
    /// [`set_mcp_service_secret_entered`] does: after both custodies are held
    /// and the installation checked, immediately before the value is
    /// removed, with the entered admission held until the removal settles
    /// and, when it states its administrator, recorded then.
    pub async fn remove_mcp_service_secret_entered(
        storage: &ResolvedStorage,
        persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
        reviewed: &McpServerConfig,
        env: &str,
        removal: McpServiceSecretRemoval,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<(), McpServiceSecretUpdateError> {
        let reference = reference(storage, reviewed, env)?;
        let document = realm_document(storage)?;
        let reviewed_owned = reviewed.clone();
        let meaning = reference.meaning().to_owned();
        update(
            persistence,
            document,
            reference,
            move |installed| match removal {
                McpServiceSecretRemoval::Current if installed == Some(&reviewed_owned) => Ok(()),
                McpServiceSecretRemoval::Current => Err(
                    McpServiceSecretUpdateError::DefinitionChanged(reviewed_owned.name),
                ),
                McpServiceSecretRemoval::Obsolete
                    if installed
                        .and_then(McpServerConfig::stdio_launch_meaning)
                        .is_some_and(|current| current == meaning) =>
                {
                    Err(McpServiceSecretUpdateError::StillCurrent(
                        reviewed_owned.name,
                    ))
                }
                McpServiceSecretRemoval::Obsolete => Ok(()),
            },
            Write::Clear,
            entry,
        )
        .await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
