//! Realm-configured connector OAuth clients.
//!
//! A realm declares its connector clients in its config document
//! (`[realm.<realm>.connector_client.<profile>]`). This module is the one
//! loader that turns the realm's effective configuration into the connector
//! owner's client-auth profiles, and the source every host installs so the
//! owner reads the current profiles at each login and refresh. Its writers
//! ([`put_connector_client`], [`rotate_connector_client_secret`],
//! [`remove_connector_client`]) change a client's document entry and its
//! secret as one guarded operation under the realm's admission fences.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use meerkat_core::connection::{RealmChain, RealmId};
use meerkat_core::{
    AdmissionGuard, Config, ConfigError, ConfigRuntime, ConnectorClientAuthMethod,
    ConnectorClientConfig,
};
use meerkat_providers::connector_login::{
    ConnectorClientAuthProfile, ConnectorClientAuthSource, ConnectorClientSecret,
    ConnectorLoginError, clear_connector_client_secret, load_connector_client_secret,
    save_connector_client_secret,
};

/// The generation sidecar of a realm's own config document, beside it.
const CONFIG_STATE_FILE: &str = "config_state.json";

/// The connector client-auth profiles `realm` declares in its effective
/// (realm-chain composed) `config`. The nearest realm defining an id wins.
/// Each profile is validated as the connector owner would.
pub fn connector_client_auth_profiles(
    config: &Config,
    realm: &RealmId,
) -> Result<Vec<ConnectorClientAuthProfile>, ConnectorLoginError> {
    let chain = RealmChain::resolve(config, realm)
        .map_err(|error| ConnectorLoginError::ClientConfig(error.to_string()))?;
    let mut profiles = BTreeMap::new();
    for owner in chain.realms() {
        let Some(section) = config.realm.get(owner.as_str()) else {
            continue;
        };
        for (id, client) in &section.connector_client {
            if profiles.contains_key(id) {
                continue;
            }
            let profile = connector_client_profile(owner, id, client)?;
            profile.validate().map_err(|_| {
                ConnectorLoginError::ClientConfig(format!(
                    "connector client '{id}' in realm '{owner}' is invalid"
                ))
            })?;
            profiles.insert(id.clone(), profile);
        }
    }
    Ok(profiles.into_values().collect())
}

fn connector_client_profile(
    owner: &RealmId,
    id: &str,
    client: &ConnectorClientConfig,
) -> Result<ConnectorClientAuthProfile, ConnectorLoginError> {
    // The secret belongs to the realm defining the client, so a child realm
    // inheriting it uses the same secret.
    let client_secret = match client.client_auth {
        ConnectorClientAuthMethod::None => None,
        ConnectorClientAuthMethod::ClientSecretPost => Some(ConnectorClientSecret::RealmStore {
            realm: owner.clone(),
        }),
    };
    Ok(ConnectorClientAuthProfile {
        id: id.to_owned(),
        issuer: client.issuer.clone(),
        client: client.client.clone(),
        client_secret,
        authorize_params: client.authorize_params.clone(),
        resource_indicator: client.resource_indicator,
        protocol: client.protocol.clone(),
        registered_redirect_uris: client.registered_redirect_uris.clone(),
    })
}

type EffectiveRealmConfig =
    Arc<dyn Fn() -> BoxFuture<'static, Result<Option<(Config, RealmId)>, String>> + Send + Sync>;

/// The realm's connector clients as a [`ConnectorClientAuthSource`]: each
/// read composes the realm's current effective configuration, so profiles
/// added or changed after the host was built apply on the next use.
#[derive(Clone)]
pub struct RealmConnectorClients {
    effective: EffectiveRealmConfig,
}

impl RealmConnectorClients {
    /// Read `realm`'s effective configuration from `source` (its own document
    /// folded over its parent chain).
    pub fn from_realm_source(
        source: Arc<dyn meerkat_core::RealmConfigSource>,
        realm: RealmId,
    ) -> Self {
        Self {
            effective: Arc::new(move || {
                let reader = meerkat_core::EffectiveConfigReader::new(Arc::clone(&source));
                let realm = realm.clone();
                Box::pin(async move {
                    let config = reader
                        .effective_config(&realm)
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok(Some((config, realm)))
                })
            }),
        }
    }

    /// Read the effective configuration through `read`, which yields the
    /// composed config and its head realm, or `None` when the host serves no
    /// realm. Only the MCP auth-resolver install path builds one this way.
    #[cfg(feature = "mcp")]
    pub(crate) fn from_effective_config(
        read: impl Fn() -> BoxFuture<'static, Result<Option<(Config, RealmId)>, String>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            effective: Arc::new(read),
        }
    }
}

#[async_trait::async_trait]
impl ConnectorClientAuthSource for RealmConnectorClients {
    async fn current_profiles(
        &self,
    ) -> Result<Vec<ConnectorClientAuthProfile>, ConnectorLoginError> {
        match (self.effective)()
            .await
            .map_err(ConnectorLoginError::ClientConfig)?
        {
            Some((config, realm)) => connector_client_auth_profiles(&config, &realm),
            None => Ok(Vec::new()),
        }
    }
}

/// How [`put_connector_client`] treats the client's secret.
pub enum ConnectorClientSecretUpdate {
    /// Keep the stored secret. A client that sends a secret must already
    /// have one.
    Keep,
    /// Store this secret (replacing any stored one).
    Set(String),
}

/// Why a connector client could not be written, or how far it got.
#[derive(Debug, thiserror::Error)]
pub enum ConnectorClientUpdateError {
    #[error("connector client '{id}' is invalid: {reason}")]
    Invalid { id: String, reason: String },
    #[error(
        "connector client '{0}' sends a client secret but none is stored for its exact definition; supply one"
    )]
    SecretRequired(String),
    #[error("connector client '{0}' sends no client secret")]
    SecretNotUsed(String),
    #[error("connector client '{id}' is not configured in realm '{realm}'")]
    Unknown { id: String, realm: RealmId },
    #[error(transparent)]
    Config(#[from] meerkat_core::ConfigError),
    #[error(transparent)]
    Store(#[from] ConnectorLoginError),
    /// The profile's update ownership, or the realm's admission fences (held
    /// by an admission or another fenced update, or unusable), could not be
    /// acquired; the update did not start and wrote nothing.
    #[error("connector client update ownership is unavailable: {0}")]
    Ownership(String),
    /// The coordinator lost the update's outcome (for example it reports
    /// cancellation). The update may have run, fully or in part.
    #[error("connector client update outcome is unknown: {0}")]
    OutcomeUnknown(String),
    /// The secret for this exact definition was stored, then the
    /// configuration write failed. An existing definition with the same
    /// meaning may already use that secret, and the store error does not say
    /// whether the configuration was written.
    #[error(
        "connector client '{id}': the secret was stored but the configuration write failed and may not have been applied: {error}"
    )]
    ConfigFailedAfterSecretStored {
        id: String,
        error: meerkat_core::ConfigError,
    },
    /// The configuration was written, then removing the secret it no longer
    /// uses failed: the configuration is in effect and that unused secret
    /// remains stored.
    #[error(
        "connector client '{id}': the configuration was written but its unused secret could not be removed: {error}"
    )]
    SecretCleanupFailed {
        id: String,
        error: ConnectorLoginError,
    },
}

/// One guarded update of `realm`'s connector client `id`: its config
/// document entry and its secret change under one admission guard.
///
/// 1. Before entry, the update takes, in the global order and with the
///    interactive wait, the fences of the realm's own document by kind (R of
///    the realm, or the user document fence U for `global`:
///    [`ConfigRuntime::admission_fences`]) and the credential fence C of the
///    realm that the persistence's coordinator names. A held or unusable
///    fence refuses as [`ConnectorClientUpdateError::Ownership`]: nothing
///    started and nothing is held.
/// 2. Entered, a task this function spawns owns the guard. It enters the
///    coordinator's per-profile key with `with_exclusive_mutation_under`,
///    so every put, rotation and removal of that profile runs one at a time
///    and an older update can never clear a secret a newer one published.
///    `update` writes the secret, and the document through the runtime's
///    `patch_under`, under that guard.
/// 3. Settled, the task ends and the guard drops; only then does this
///    future resolve. A caller that stops waiting after step 1 does not stop
///    step 2, and the fences are released only at its end.
async fn guarded_profile_update<F, Fut>(
    documents: Arc<meerkat_store::FilesystemRealmConfigSource>,
    persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
    realm: &RealmId,
    id: &str,
    update: F,
) -> Result<(), ConnectorClientUpdateError>
where
    F: FnOnce(Arc<AdmissionGuard>, Arc<ConfigRuntime>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), ConnectorClientUpdateError>> + Send + 'static,
{
    let document = documents.config_doc_path(realm);
    let state_path = document.with_file_name(CONFIG_STATE_FILE);
    let documents: Arc<dyn meerkat_core::RealmConfigSource> = documents;
    let runtime = Arc::new(ConfigRuntime::new(
        Arc::new(
            meerkat_core::FileConfigStore::new(document, meerkat_models::canonical())
                .with_realm_chain(realm.clone(), documents),
        ),
        state_path,
    ));
    let mut fences = runtime.admission_fences();
    if let Some(credentials) = persistence
        .refresh_coordinator()
        .credential_fence(realm)
        .map_err(|error| ConnectorClientUpdateError::Ownership(error.to_string()))?
    {
        fences.push(credentials);
    }
    let guard = AdmissionGuard::acquire(
        fences,
        meerkat_core::realm_admission::INTERACTIVE_FENCE_WAIT,
    )
    .await
    .map_err(|refusal| ConnectorClientUpdateError::Ownership(refusal.to_string()))?;
    let persistence = persistence.clone();
    let (realm, id) = (realm.clone(), id.to_owned());
    tokio::spawn(async move {
        with_profile_update_under(guard, &persistence, &realm, &id, move |guard| {
            update(guard, runtime)
        })
        .await
    })
    .await
    .unwrap_or_else(|error| {
        Err(ConnectorClientUpdateError::OutcomeUnknown(format!(
            "the update's task did not run to its end: {error}"
        )))
    })
}

/// Run one update of `realm`'s connector client `id` under the persistence's
/// exclusive-mutation coordinator, keyed by the profile id, for an operation
/// that already holds the realm's credential fence in `guard`. `update`
/// receives the guard the coordinator hands its closure, which its writes
/// take.
async fn with_profile_update_under<F, Fut>(
    guard: Arc<AdmissionGuard>,
    persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
    realm: &RealmId,
    id: &str,
    update: F,
) -> Result<(), ConnectorClientUpdateError>
where
    F: FnOnce(Arc<AdmissionGuard>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), ConnectorClientUpdateError>> + Send + 'static,
{
    use meerkat_core::auth::{CredentialMutationFn, CredentialMutationOutcome, TokenStoreAccess};
    let key = meerkat_core::auth::TokenKey::connector_client_update(realm.clone(), id).map_err(
        |error| ConnectorClientUpdateError::Invalid {
            id: id.to_owned(),
            reason: error.to_string(),
        },
    )?;
    let outcome = Arc::new(std::sync::Mutex::new(None));
    let slot = Arc::clone(&outcome);
    let mutation: CredentialMutationFn = Box::new(move |guard| {
        Box::pin(async move {
            let result = update(guard).await;
            *slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
            Ok(CredentialMutationOutcome::Cleared)
        })
    });
    let admitted = persistence
        .refresh_coordinator()
        .with_exclusive_mutation_under(guard, key, TokenStoreAccess::Interactive, mutation)
        .await;
    let result = outcome
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    match (admitted, result) {
        // The update ran to its own typed result.
        (_, Some(result)) => result,
        // Ownership was never acquired: the update did not start.
        (Err(meerkat_core::auth::CredentialMutationError::LockFailed(error)), None) => {
            Err(ConnectorClientUpdateError::Ownership(error))
        }
        // Admission was refused while earlier work for the key still runs,
        // or the coordinator refused the operation's guard: the update did
        // not start.
        (
            Err(
                error @ (meerkat_core::auth::CredentialMutationError::CredentialAccess { .. }
                | meerkat_core::auth::CredentialMutationError::AdmissionInProgress
                | meerkat_core::auth::CredentialMutationError::AdmissionFenceUnavailable(_)),
            ),
            None,
        ) => Err(ConnectorClientUpdateError::Ownership(error.to_string())),
        // Anything else (including cancellation) lost the outcome.
        (Err(error), None) => Err(ConnectorClientUpdateError::OutcomeUnknown(
            error.to_string(),
        )),
        (Ok(_), None) => Err(ConnectorClientUpdateError::OutcomeUnknown(
            "the coordinator returned without the update's result".to_owned(),
        )),
    }
}

/// The non-secret profile binding of `realm`'s client `id` as defined.
fn client_binding(
    realm: &RealmId,
    id: &str,
    client: &ConnectorClientConfig,
) -> Result<String, ConnectorClientUpdateError> {
    let profile = connector_client_profile(realm, id, client).map_err(|error| {
        ConnectorClientUpdateError::Invalid {
            id: id.to_owned(),
            reason: error.to_string(),
        }
    })?;
    profile
        .validate()
        .map_err(|error| ConnectorClientUpdateError::Invalid {
            id: id.to_owned(),
            reason: error.to_string(),
        })?;
    Ok(profile.binding())
}

/// The merge patch writing `client` (or removing it, for `None`) as `realm`'s
/// connector client `id`. Every owned field is written explicitly, so the
/// result is exactly `client` whatever definition was there before.
fn connector_client_patch(
    realm: &RealmId,
    id: &str,
    client: Option<&ConnectorClientConfig>,
) -> Result<meerkat_core::ConfigDelta, ConnectorClientUpdateError> {
    let value = match client {
        Some(client) => {
            let mut value = serde_json::to_value(client).map_err(|error| {
                ConnectorClientUpdateError::Invalid {
                    id: id.to_owned(),
                    reason: error.to_string(),
                }
            })?;
            if let Some(fields) = value.as_object_mut() {
                fields
                    .entry("authorize_params")
                    .or_insert_with(|| serde_json::json!([]));
                fields
                    .entry("registered_redirect_uris")
                    .or_insert_with(|| serde_json::json!([]));
                fields.entry("protocol").or_insert(serde_json::Value::Null);
            }
            value
        }
        None => serde_json::Value::Null,
    };
    Ok(meerkat_core::ConfigDelta(serde_json::json!({
        "realm": { realm.as_str(): { "connector_client": { id: value } } }
    })))
}

/// Configure `realm`'s connector client `id`, writing the client to the
/// realm's own config document in `documents` (never a composed view) and
/// its secret to the realm credential store, as one guarded operation.
///
/// Before anything starts, the update takes the admission fences of the
/// realm's own document and the realm's credential fence, in the global
/// order and waiting up to the interactive bound for a holder; a held or
/// unusable fence refuses it with nothing written
/// ([`ConnectorClientUpdateError::Ownership`]). The update then runs on its
/// own task, one at a time per client: a caller that stops waiting does not
/// stop it, and the fences are released only after its last write.
///
/// A stored secret is bound to the client's exact non-secret meaning
/// (issuer, client, mode, parameters, protocol, redirects): `Keep` keeps only
/// a secret stored for this exact definition, so a retargeted client never
/// inherits the previous client's secret. A new secret is stored before the
/// configuration is written, and a secret the new definition no longer uses
/// is removed after. The two stores are written separately, not atomically:
/// if the process stops in between, a configured client without its exact
/// secret refuses each login and refresh with
/// [`ConnectorLoginError::ClientSecretUnavailable`], and a failure after the
/// first write is reported as the partial outcome it is.
pub async fn put_connector_client(
    documents: Arc<meerkat_store::FilesystemRealmConfigSource>,
    persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
    realm: &RealmId,
    id: &str,
    client: ConnectorClientConfig,
    secret: ConnectorClientSecretUpdate,
) -> Result<(), ConnectorClientUpdateError> {
    let token_store = persistence.token_store();
    let (realm_owned, id_owned) = (realm.clone(), id.to_owned());
    guarded_profile_update(
        documents,
        persistence,
        realm,
        id,
        move |guard, runtime| async move {
            put_unowned(
                &guard,
                &runtime,
                token_store.as_ref(),
                &realm_owned,
                &id_owned,
                client,
                secret,
            )
            .await
        },
    )
    .await
}

/// The realm's own document as `runtime` reads it now.
async fn own_config(runtime: &ConfigRuntime) -> Result<Config, ConfigError> {
    Ok(runtime.get().await.map_err(ConfigError::from)?.config)
}

async fn put_unowned(
    guard: &Arc<AdmissionGuard>,
    runtime: &ConfigRuntime,
    token_store: &dyn meerkat_core::auth::TokenStore,
    realm: &RealmId,
    id: &str,
    client: ConnectorClientConfig,
    secret: ConnectorClientSecretUpdate,
) -> Result<(), ConnectorClientUpdateError> {
    let binding = client_binding(realm, id, &client)?;
    let sends_secret = client.client_auth == ConnectorClientAuthMethod::ClientSecretPost;
    let own = own_config(runtime).await?;
    let previous = own
        .realm
        .get(realm.as_str())
        .and_then(|section| section.connector_client.get(id))
        .cloned();
    let previous_binding = previous
        .as_ref()
        .filter(|previous| previous.client_auth == ConnectorClientAuthMethod::ClientSecretPost)
        .map(|previous| client_binding(realm, id, previous))
        .transpose()?;
    let mut stored_new_secret = false;
    match (&secret, sends_secret) {
        (ConnectorClientSecretUpdate::Set(value), true) => {
            save_connector_client_secret(guard, token_store, realm, &binding, value).await?;
            stored_new_secret = true;
        }
        (ConnectorClientSecretUpdate::Keep, true) => {
            if load_connector_client_secret(token_store, realm, &binding)
                .await?
                .is_none()
            {
                return Err(ConnectorClientUpdateError::SecretRequired(id.to_owned()));
            }
        }
        (ConnectorClientSecretUpdate::Set(_), false) => {
            return Err(ConnectorClientUpdateError::SecretNotUsed(id.to_owned()));
        }
        (ConnectorClientSecretUpdate::Keep, false) => {}
    }
    let patch = connector_client_patch(realm, id, Some(&client))?;
    if let Err(error) = runtime.patch_under(guard, patch, None).await {
        let error = ConfigError::from(error);
        return Err(if stored_new_secret {
            ConnectorClientUpdateError::ConfigFailedAfterSecretStored {
                id: id.to_owned(),
                error,
            }
        } else {
            error.into()
        });
    }
    if let Some(previous_binding) = previous_binding
        && (!sends_secret || previous_binding != binding)
    {
        clear_unreferenced_secret(guard, runtime, token_store, realm, id, &previous_binding)
            .await?;
    }
    Ok(())
}

/// Remove the secret stored for `realm`'s client binding `binding` unless the
/// realm's own configuration names that exact client again (a concurrent
/// writer may have restored it since this update read the configuration).
async fn clear_unreferenced_secret(
    guard: &AdmissionGuard,
    runtime: &ConfigRuntime,
    token_store: &dyn meerkat_core::auth::TokenStore,
    realm: &RealmId,
    id: &str,
    binding: &str,
) -> Result<(), ConnectorClientUpdateError> {
    let cleanup_failed = |error| ConnectorClientUpdateError::SecretCleanupFailed {
        id: id.to_owned(),
        error,
    };
    let current = own_config(runtime)
        .await
        .map_err(|error| cleanup_failed(ConnectorLoginError::ClientConfig(error.to_string())))?;
    let still_named = current
        .realm
        .get(realm.as_str())
        .and_then(|section| section.connector_client.get(id))
        .filter(|client| client.client_auth == ConnectorClientAuthMethod::ClientSecretPost)
        .is_some_and(|client| {
            client_binding(realm, id, client).is_ok_and(|current| current == binding)
        });
    if still_named {
        return Ok(());
    }
    clear_connector_client_secret(guard, token_store, realm, binding)
        .await
        .map_err(cleanup_failed)
}

/// Replace the stored secret of `realm`'s connector client `id`, which must be
/// defined in the realm's own config document in `documents` and send a
/// secret, as one guarded operation (see [`put_connector_client`]). The
/// secret is stored for the client's current exact definition; the next code
/// exchange or refresh uses it, and credentials obtained under the old one
/// stay valid.
pub async fn rotate_connector_client_secret(
    documents: Arc<meerkat_store::FilesystemRealmConfigSource>,
    persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
    realm: &RealmId,
    id: &str,
    secret: &str,
) -> Result<(), ConnectorClientUpdateError> {
    let token_store = persistence.token_store();
    let (realm_owned, id_owned, secret) = (realm.clone(), id.to_owned(), secret.to_owned());
    guarded_profile_update(
        documents,
        persistence,
        realm,
        id,
        move |guard, runtime| async move {
            rotate_unowned(
                &guard,
                &runtime,
                token_store.as_ref(),
                &realm_owned,
                &id_owned,
                &secret,
            )
            .await
        },
    )
    .await
}

async fn rotate_unowned(
    guard: &AdmissionGuard,
    runtime: &ConfigRuntime,
    token_store: &dyn meerkat_core::auth::TokenStore,
    realm: &RealmId,
    id: &str,
    secret: &str,
) -> Result<(), ConnectorClientUpdateError> {
    let own = own_config(runtime).await?;
    let client = own
        .realm
        .get(realm.as_str())
        .and_then(|section| section.connector_client.get(id))
        .ok_or_else(|| ConnectorClientUpdateError::Unknown {
            id: id.to_owned(),
            realm: realm.clone(),
        })?;
    if client.client_auth != ConnectorClientAuthMethod::ClientSecretPost {
        return Err(ConnectorClientUpdateError::SecretNotUsed(id.to_owned()));
    }
    let binding = client_binding(realm, id, client)?;
    save_connector_client_secret(guard, token_store, realm, &binding, secret).await?;
    Ok(())
}

/// Remove `realm`'s connector client `id` from its own config document in
/// `documents`, then its stored secret, as one guarded operation (see
/// [`put_connector_client`]). Credentials minted under it stop refreshing.
pub async fn remove_connector_client(
    documents: Arc<meerkat_store::FilesystemRealmConfigSource>,
    persistence: &meerkat_providers::auth_store::ProviderAuthPersistence,
    realm: &RealmId,
    id: &str,
) -> Result<(), ConnectorClientUpdateError> {
    let token_store = persistence.token_store();
    let (realm_owned, id_owned) = (realm.clone(), id.to_owned());
    guarded_profile_update(
        documents,
        persistence,
        realm,
        id,
        move |guard, runtime| async move {
            remove_unowned(
                &guard,
                &runtime,
                token_store.as_ref(),
                &realm_owned,
                &id_owned,
            )
            .await
        },
    )
    .await
}

async fn remove_unowned(
    guard: &Arc<AdmissionGuard>,
    runtime: &ConfigRuntime,
    token_store: &dyn meerkat_core::auth::TokenStore,
    realm: &RealmId,
    id: &str,
) -> Result<(), ConnectorClientUpdateError> {
    let own = own_config(runtime).await?;
    let previous_binding = own
        .realm
        .get(realm.as_str())
        .and_then(|section| section.connector_client.get(id))
        .filter(|previous| previous.client_auth == ConnectorClientAuthMethod::ClientSecretPost)
        .map(|previous| client_binding(realm, id, previous))
        .transpose()?;
    runtime
        .patch_under(guard, connector_client_patch(realm, id, None)?, None)
        .await
        .map_err(ConfigError::from)?;
    if let Some(previous_binding) = previous_binding {
        clear_unreferenced_secret(guard, runtime, token_store, realm, id, &previous_binding)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const DOCUMENT: &str = r#"
[realm.global.connector_client.shared]
issuer = "https://issuer.global.example"
client = "global-client"
authorize_params = [["access_type", "offline"], ["prompt", "consent"]]

[realm.global.connector_client.overridden]
issuer = "https://issuer.global.example"
client = "global-client"

[realm.child.connector_client.overridden]
issuer = "https://issuer.child.example"
client = "child-client"
resource_indicator = false
protocol = "slack"
registered_redirect_uris = ["https://host.example/callback"]
"#;

    fn config(document: &str) -> Config {
        Config::from_persisted_toml(document)
            .expect("config parses")
            .0
    }

    fn realm(id: &str) -> RealmId {
        RealmId::parse(id).expect("realm id")
    }

    #[test]
    fn the_nearest_realm_defining_a_client_wins_and_fields_map_in_order() {
        let profiles =
            connector_client_auth_profiles(&config(DOCUMENT), &realm("child")).expect("profiles");
        let by_id: BTreeMap<_, _> = profiles
            .iter()
            .map(|profile| (profile.id.as_str(), profile))
            .collect();
        assert_eq!(by_id.len(), 2);
        let overridden = by_id["overridden"];
        assert_eq!(overridden.issuer, "https://issuer.child.example");
        assert_eq!(overridden.client, "child-client");
        assert!(!overridden.resource_indicator);
        assert_eq!(overridden.protocol.as_deref(), Some("slack"));
        assert_eq!(
            overridden.registered_redirect_uris,
            vec!["https://host.example/callback".to_owned()]
        );
        let shared = by_id["shared"];
        assert!(shared.resource_indicator, "the default sends the resource");
        assert!(shared.client_secret.is_none());
        assert_eq!(
            shared.authorize_params,
            vec![
                ("access_type".to_owned(), "offline".to_owned()),
                ("prompt".to_owned(), "consent".to_owned()),
            ]
        );
        let global = connector_client_auth_profiles(&config(DOCUMENT), &RealmId::global())
            .expect("global profiles");
        assert!(global.iter().any(|profile| profile.id == "overridden"
            && profile.issuer == "https://issuer.global.example"));
    }

    #[test]
    fn a_secret_client_reads_the_defining_realms_store() {
        let document = r#"
[realm.global.connector_client.desktop]
issuer = "https://issuer.example"
client = "client"
client_auth = "client_secret_post"
"#;
        let profiles =
            connector_client_auth_profiles(&config(document), &realm("child")).expect("profiles");
        assert_eq!(
            profiles[0].client_secret,
            Some(ConnectorClientSecret::RealmStore {
                realm: RealmId::global()
            }),
            "an inherited client uses its defining realm's secret"
        );
    }

    #[test]
    fn an_invalid_client_is_a_configuration_error_naming_it() {
        let document = r#"
[realm.global.connector_client.broken]
issuer = "https://issuer.example"
client = "client"
authorize_params = [["client_secret", "smuggled"]]
"#;
        let error = connector_client_auth_profiles(&config(document), &RealmId::global())
            .expect_err("reserved parameter");
        let ConnectorLoginError::ClientConfig(detail) = error else {
            panic!("expected a configuration error: {error:?}");
        };
        assert!(detail.contains("broken"), "{detail}");
        assert!(!detail.contains("smuggled"), "{detail}");
    }

    #[test]
    fn an_unknown_client_field_is_refused_at_load() {
        let document = r#"
[realm.global.connector_client.typo]
issuer = "https://issuer.example"
client = "client"
client_secret = "never-in-config"
"#;
        assert!(Config::from_persisted_toml(document).is_err());
    }

    #[tokio::test]
    async fn put_rotate_and_remove_keep_config_and_secret_consistent() {
        use meerkat_providers::auth_store::{EphemeralTokenStore, TokenStore};
        let root = tempfile::tempdir().unwrap();
        let documents = realm_documents(root.path());
        let secrets = Arc::new(EphemeralTokenStore::new());
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::clone(&secrets) as Arc<dyn TokenStore>,
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let realm = RealmId::global();
        let client = |client_auth, protocol: Option<&str>| ConnectorClientConfig {
            issuer: "https://issuer.example".into(),
            client: "client".into(),
            client_auth,
            authorize_params: vec![("prompt".into(), "consent".into())],
            resource_indicator: true,
            protocol: protocol.map(str::to_owned),
            registered_redirect_uris: Vec::new(),
        };
        let stored = || stored_client_secret(&documents, secrets.as_ref(), &realm, "desktop");
        async fn stored_secret_rows(secrets: &EphemeralTokenStore) -> usize {
            secrets
                .list()
                .await
                .unwrap()
                .iter()
                .filter(|key| key.is_connector_client_secret())
                .count()
        }

        // A client sending a secret needs one: nothing is written.
        let refused = put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            client(ConnectorClientAuthMethod::ClientSecretPost, None),
            ConnectorClientSecretUpdate::Keep,
        )
        .await;
        assert!(matches!(
            refused,
            Err(ConnectorClientUpdateError::SecretRequired(_))
        ));
        assert!(
            connector_client_auth_profiles(&stored_document(&documents, &realm), &realm)
                .unwrap()
                .is_empty()
        );

        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            client(ConnectorClientAuthMethod::ClientSecretPost, Some("slack")),
            ConnectorClientSecretUpdate::Set("secret-one".into()),
        )
        .await
        .unwrap();
        let profiles =
            connector_client_auth_profiles(&stored_document(&documents, &realm), &realm).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].protocol.as_deref(), Some("slack"));
        assert!(
            !std::fs::read_to_string(documents.config_doc_path(&realm))
                .unwrap()
                .contains("secret-one"),
            "the secret never reaches the config document"
        );
        assert_eq!(stored().await, Some("secret-one".to_owned()));

        rotate_connector_client_secret(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            "secret-two",
        )
        .await
        .unwrap();
        assert_eq!(stored().await, Some("secret-two".to_owned()));

        // Retargeted to another client with Keep: the previous client's
        // secret is not reused, and nothing changes.
        let retargeted = ConnectorClientConfig {
            client: "replacement-client".into(),
            ..client(ConnectorClientAuthMethod::ClientSecretPost, Some("slack"))
        };
        assert!(matches!(
            put_connector_client(
                Arc::clone(&documents),
                &persistence,
                &realm,
                "desktop",
                retargeted.clone(),
                ConnectorClientSecretUpdate::Keep,
            )
            .await,
            Err(ConnectorClientUpdateError::SecretRequired(_))
        ));
        assert_eq!(stored().await, Some("secret-two".to_owned()));
        // Retargeted with its own secret: the old client's secret is removed.
        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            retargeted,
            ConnectorClientSecretUpdate::Set("replacement-secret".into()),
        )
        .await
        .unwrap();
        assert_eq!(stored().await, Some("replacement-secret".to_owned()));
        assert_eq!(stored_secret_rows(&secrets).await, 1);

        // Redefined without a protocol or a secret: the old values go.
        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            client(ConnectorClientAuthMethod::None, None),
            ConnectorClientSecretUpdate::Keep,
        )
        .await
        .unwrap();
        let profiles =
            connector_client_auth_profiles(&stored_document(&documents, &realm), &realm).unwrap();
        assert_eq!(profiles[0].protocol, None);
        assert!(profiles[0].client_secret.is_none());
        assert!(stored().await.is_none());
        assert_eq!(stored_secret_rows(&secrets).await, 0);
        assert!(matches!(
            rotate_connector_client_secret(
                Arc::clone(&documents),
                &persistence,
                &realm,
                "desktop",
                "x"
            )
            .await,
            Err(ConnectorClientUpdateError::SecretNotUsed(_))
        ));

        remove_connector_client(Arc::clone(&documents), &persistence, &realm, "desktop")
            .await
            .unwrap();
        assert!(
            connector_client_auth_profiles(&stored_document(&documents, &realm), &realm)
                .unwrap()
                .is_empty()
        );
    }

    /// A token store whose first clear waits until released, so an older
    /// update can be held between publishing its config and clearing.
    struct HeldClearStore {
        inner: meerkat_providers::auth_store::EphemeralTokenStore,
        saves: std::sync::atomic::AtomicUsize,
        clears_done: std::sync::atomic::AtomicUsize,
        entered: tokio::sync::Notify,
        release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[async_trait::async_trait]
    impl meerkat_providers::auth_store::TokenStore for HeldClearStore {
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
            self.saves.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.save(fence, key, tokens).await
        }
        async fn clear(
            &self,
            fence: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            let held = self.release.lock().await.take();
            if let Some(release) = held {
                self.entered.notify_one();
                let _ = release.await;
            }
            let cleared = self.inner.clear(fence, key).await;
            self.clears_done
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            cleared
        }
        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.list().await
        }
        fn backend_name(&self) -> &'static str {
            "held-clear"
        }
    }

    /// The in-memory coordinator, reporting each exclusive-mutation request
    /// before it waits for admission.
    struct SignalingCoordinator {
        inner: meerkat_providers::auth_store::InMemoryCoordinator,
        requested: tokio::sync::mpsc::UnboundedSender<()>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::auth::RefreshCoordinator for SignalingCoordinator {
        async fn with_exclusive_mutation(
            &self,
            key: meerkat_core::auth::TokenKey,
            mutation_fn: meerkat_core::auth::CredentialMutationFn,
        ) -> Result<
            meerkat_core::auth::CredentialMutationOutcome,
            meerkat_core::auth::CredentialMutationError,
        > {
            let _ = self.requested.send(());
            self.inner.with_exclusive_mutation(key, mutation_fn).await
        }

        async fn with_exclusive_mutation_under(
            &self,
            guard: Arc<meerkat_core::AdmissionGuard>,
            key: meerkat_core::auth::TokenKey,
            access: meerkat_core::auth::TokenStoreAccess,
            mutation_fn: meerkat_core::auth::CredentialMutationFn,
        ) -> Result<
            meerkat_core::auth::CredentialMutationOutcome,
            meerkat_core::auth::CredentialMutationError,
        > {
            let _ = self.requested.send(());
            self.inner
                .with_exclusive_mutation_under(guard, key, access, mutation_fn)
                .await
        }

        fn credential_fence(
            &self,
            realm: &RealmId,
        ) -> Result<Option<meerkat_core::AdmissionFence>, meerkat_core::auth::CredentialMutationError>
        {
            self.inner.credential_fence(realm)
        }

        async fn with_refresh(
            &self,
            key: meerkat_core::auth::TokenKey,
            refresh_fn: meerkat_core::auth::RefreshFn,
        ) -> Result<meerkat_core::auth::PersistedTokens, meerkat_core::auth::RefreshError> {
            self.inner.with_refresh(key, refresh_fn).await
        }
    }

    #[tokio::test]
    async fn an_older_held_clear_never_removes_a_newer_updates_secret() {
        use meerkat_providers::auth_store::{EphemeralTokenStore, TokenStore};
        let root = tempfile::tempdir().unwrap();
        let documents = realm_documents(root.path());
        let (release, held) = tokio::sync::oneshot::channel();
        let secrets = Arc::new(HeldClearStore {
            inner: EphemeralTokenStore::new(),
            saves: std::sync::atomic::AtomicUsize::new(0),
            clears_done: std::sync::atomic::AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Mutex::new(None),
        });
        let (requested, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::clone(&secrets) as Arc<dyn TokenStore>,
            Arc::new(SignalingCoordinator {
                inner: meerkat_providers::auth_store::InMemoryCoordinator::new(),
                requested,
            }),
        );
        let realm = RealmId::global();
        let secret_client = ConnectorClientConfig {
            issuer: "https://issuer.example".into(),
            client: "client".into(),
            client_auth: ConnectorClientAuthMethod::ClientSecretPost,
            authorize_params: Vec::new(),
            resource_indicator: true,
            protocol: None,
            registered_redirect_uris: Vec::new(),
        };
        let public_client = ConnectorClientConfig {
            client_auth: ConnectorClientAuthMethod::None,
            ..secret_client.clone()
        };
        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            secret_client.clone(),
            ConnectorClientSecretUpdate::Set("secret-one".into()),
        )
        .await
        .unwrap();

        while requests.try_recv().is_ok() {}
        // Older update: switch to a public client; it publishes its config
        // and is then held before clearing the old secret.
        *secrets.release.lock().await = Some(held);
        let older = tokio::spawn({
            let (documents, persistence, realm) =
                (Arc::clone(&documents), persistence.clone(), realm.clone());
            async move {
                put_connector_client(
                    documents,
                    &persistence,
                    &realm,
                    "desktop",
                    public_client,
                    ConnectorClientSecretUpdate::Keep,
                )
                .await
            }
        });
        secrets.entered.notified().await;
        requests
            .recv()
            .await
            .expect("the older update entered its profile's ownership");
        // Its caller leaves; the entered update still completes in order.
        older.abort();
        assert!(
            older.await.is_err_and(|error| error.is_cancelled()),
            "the older caller is gone"
        );
        let clears_before = secrets
            .clears_done
            .load(std::sync::atomic::Ordering::SeqCst);
        let saves_before = secrets.saves.load(std::sync::atomic::Ordering::SeqCst);
        // Newer update: back to the secret client with a new secret.
        let newer = tokio::spawn({
            let (documents, persistence, realm) =
                (Arc::clone(&documents), persistence.clone(), realm.clone());
            async move {
                put_connector_client(
                    documents,
                    &persistence,
                    &realm,
                    "desktop",
                    secret_client,
                    ConnectorClientSecretUpdate::Set("secret-two".into()),
                )
                .await
            }
        });
        // While the older update's cleanup is held it keeps the realm's
        // admission fences: the newer update waits for them before it
        // enters, so it has entered nothing, written nothing and not
        // finished, and it completes only after the cleanup does.
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert!(!newer.is_finished(), "the newer update has not completed");
        assert!(
            requests.try_recv().is_err(),
            "the newer update has not entered its profile's ownership"
        );
        assert_eq!(
            secrets.saves.load(std::sync::atomic::Ordering::SeqCst),
            saves_before,
            "the newer update saved nothing while the cleanup was held"
        );
        assert!(release.send(()).is_ok(), "the held clear is released");
        newer.await.unwrap().unwrap();
        assert_eq!(
            secrets
                .clears_done
                .load(std::sync::atomic::Ordering::SeqCst),
            clears_before + 1,
            "the held cleanup completed its clear"
        );

        let profiles =
            connector_client_auth_profiles(&stored_document(&documents, &realm), &realm).unwrap();
        assert_eq!(
            load_connector_client_secret(secrets.as_ref(), &realm, &profiles[0].binding())
                .await
                .unwrap(),
            Some("secret-two".to_owned()),
            "the last successful update keeps its own secret"
        );
    }

    #[tokio::test]
    async fn an_update_clears_previously_non_empty_lists() {
        use meerkat_providers::auth_store::{EphemeralTokenStore, TokenStore};
        let root = tempfile::tempdir().unwrap();
        let documents = realm_documents(root.path());
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(EphemeralTokenStore::new()) as Arc<dyn TokenStore>,
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let realm = RealmId::global();
        let full = ConnectorClientConfig {
            issuer: "https://issuer.example".into(),
            client: "client".into(),
            client_auth: ConnectorClientAuthMethod::None,
            authorize_params: vec![
                ("access_type".into(), "offline".into()),
                ("prompt".into(), "consent".into()),
            ],
            resource_indicator: true,
            protocol: None,
            registered_redirect_uris: vec!["https://host.example/callback".into()],
        };
        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            full.clone(),
            ConnectorClientSecretUpdate::Keep,
        )
        .await
        .unwrap();
        let profiles =
            connector_client_auth_profiles(&stored_document(&documents, &realm), &realm).unwrap();
        assert_eq!(profiles[0].authorize_params.len(), 2);
        assert_eq!(profiles[0].registered_redirect_uris.len(), 1);

        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            ConnectorClientConfig {
                authorize_params: Vec::new(),
                registered_redirect_uris: Vec::new(),
                ..full
            },
            ConnectorClientSecretUpdate::Keep,
        )
        .await
        .unwrap();
        let profiles =
            connector_client_auth_profiles(&stored_document(&documents, &realm), &realm).unwrap();
        assert!(profiles[0].authorize_params.is_empty(), "{:?}", profiles[0]);
        assert!(
            profiles[0].registered_redirect_uris.is_empty(),
            "{:?}",
            profiles[0]
        );
    }

    /// Replacing a profile with empty lists and no protocol clears what the
    /// stored profile had, whether that profile was written through this API
    /// or directly in the document: the patch is complete on its own, not
    /// shaped by a prior read.
    #[tokio::test]
    async fn a_replacement_clears_previously_set_lists_and_protocol() {
        use meerkat_providers::auth_store::{EphemeralTokenStore, TokenStore};
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(EphemeralTokenStore::new()) as Arc<dyn TokenStore>,
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let realm = RealmId::global();
        let populated = ConnectorClientConfig {
            issuer: "https://issuer.example".into(),
            client: "client".into(),
            client_auth: ConnectorClientAuthMethod::None,
            authorize_params: vec![("prompt".into(), "consent".into())],
            resource_indicator: true,
            protocol: Some("slack".into()),
            registered_redirect_uris: vec!["https://app.example/callback".into()],
        };
        let emptied = ConnectorClientConfig {
            authorize_params: Vec::new(),
            protocol: None,
            registered_redirect_uris: Vec::new(),
            ..populated.clone()
        };
        let stored = |config: &Config| {
            config
                .realm
                .get(realm.as_str())
                .and_then(|section| section.connector_client.get("desktop"))
                .cloned()
        };

        // Written through this API, then replaced through it.
        let through_api = tempfile::tempdir().unwrap();
        let documents = realm_documents(through_api.path());
        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            populated.clone(),
            ConnectorClientSecretUpdate::Keep,
        )
        .await
        .unwrap();
        assert_eq!(
            stored(&stored_document(&documents, &realm)),
            Some(populated.clone())
        );
        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            emptied.clone(),
            ConnectorClientSecretUpdate::Keep,
        )
        .await
        .unwrap();
        assert_eq!(
            stored(&stored_document(&documents, &realm)),
            Some(emptied.clone()),
            "the replacement clears both lists and the protocol"
        );

        // Written directly in the document, then replaced through this API.
        let written_directly = tempfile::tempdir().unwrap();
        let documents = realm_documents(written_directly.path());
        let path = documents.config_doc_path(&realm);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
[realm.global.connector_client.desktop]
issuer = "https://issuer.example"
client = "client"
authorize_params = [["prompt", "consent"]]
protocol = "slack"
registered_redirect_uris = ["https://app.example/callback"]
"#,
        )
        .unwrap();
        assert_eq!(
            stored(&stored_document(&documents, &realm)),
            Some(populated)
        );
        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &realm,
            "desktop",
            emptied.clone(),
            ConnectorClientSecretUpdate::Keep,
        )
        .await
        .unwrap();
        assert_eq!(
            stored(&stored_document(&documents, &realm)),
            Some(emptied),
            "the replacement of a directly written profile clears both lists and the protocol"
        );
        let document = std::fs::read_to_string(&path).unwrap();
        assert!(
            !document.contains("consent")
                && !document.contains("app.example")
                && !document.contains("slack"),
            "no old value survives in the document: {document}"
        );
    }

    // The guarded writers: one admission guard over the realm's own
    // document and its credential fence covers the whole config and secret
    // change; a held fence refuses before anything is written; a caller that
    // stops waiting after entry leaves the update to settle, and the fences
    // are released only after its last write.

    /// `root`'s realm documents: each realm's own document under
    /// `root/realms`, the user document under `root/user`.
    fn realm_documents(root: &std::path::Path) -> Arc<meerkat_store::FilesystemRealmConfigSource> {
        Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
            root.join("realms"),
            root.join("user").join("config.toml"),
            meerkat_models::canonical(),
        ))
    }

    /// `realm`'s own document as stored, or the default when there is none.
    fn stored_document(
        documents: &meerkat_store::FilesystemRealmConfigSource,
        realm: &RealmId,
    ) -> Config {
        match std::fs::read_to_string(documents.config_doc_path(realm)) {
            Ok(text) => config(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(error) => panic!("the realm document is unreadable: {error}"),
        }
    }

    /// The generation the realm document's sidecar records, 0 before any.
    fn document_generation(
        documents: &meerkat_store::FilesystemRealmConfigSource,
        realm: &RealmId,
    ) -> u64 {
        let sidecar = documents
            .config_doc_path(realm)
            .with_file_name("config_state.json");
        match std::fs::read(&sidecar) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                .expect("the sidecar parses")["generation"]
                .as_u64()
                .expect("the sidecar records a generation"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("the sidecar is unreadable: {error}"),
        }
    }

    /// The secret `realm`'s client `id` reads in its own document, if any.
    async fn stored_client_secret(
        documents: &meerkat_store::FilesystemRealmConfigSource,
        secrets: &dyn meerkat_providers::auth_store::TokenStore,
        realm: &RealmId,
        id: &str,
    ) -> Option<String> {
        let profiles = connector_client_auth_profiles(&stored_document(documents, realm), realm)
            .expect("the stored clients are valid");
        let profile = profiles.iter().find(|profile| profile.id == id)?;
        let Some(ConnectorClientSecret::RealmStore { realm }) = &profile.client_secret else {
            return None;
        };
        load_connector_client_secret(secrets, realm, &profile.binding())
            .await
            .expect("the secret store reads")
    }

    fn secret_client() -> ConnectorClientConfig {
        ConnectorClientConfig {
            issuer: "https://issuer.example".into(),
            client: "client".into(),
            client_auth: ConnectorClientAuthMethod::ClientSecretPost,
            authorize_params: Vec::new(),
            resource_indicator: true,
            protocol: None,
            registered_redirect_uris: Vec::new(),
        }
    }

    /// A persisted credential store under `root/credentials` that records,
    /// for each write, whether its guard covers every fence of `fences`.
    struct CoverageStore {
        inner: meerkat_providers::auth_store::FileTokenStore,
        fences: Vec<meerkat_core::AdmissionFence>,
        writes: std::sync::Mutex<Vec<bool>>,
    }

    impl CoverageStore {
        fn record(&self, guard: &meerkat_core::AdmissionGuard) {
            let covered = self.fences.iter().all(|fence| guard.covers(fence).is_ok());
            self.writes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(covered);
        }
    }

    #[async_trait::async_trait]
    impl meerkat_providers::auth_store::TokenStore for CoverageStore {
        async fn load(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.load(key).await
        }
        async fn save(
            &self,
            guard: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
            tokens: &meerkat_core::auth::PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.record(guard);
            self.inner.save(guard, key, tokens).await
        }
        async fn clear(
            &self,
            guard: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.record(guard);
            self.inner.clear(guard, key).await
        }
        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.list().await
        }
        fn backend_name(&self) -> &'static str {
            "coverage"
        }
    }

    /// With no holder, a put, a rotation and a removal each take one guard
    /// that covers both the realm document's fence and the realm's
    /// credential fence: every secret write runs under it, and every
    /// document write moves the document's generation.
    #[tokio::test]
    async fn put_rotate_remove_take_one_guard_for_config_and_secret() {
        use meerkat_providers::auth_store::{
            FileLockCoordinator, FileTokenStore, ProviderAuthPersistence, TokenStore,
        };
        let root = tempfile::tempdir().unwrap();
        let documents = realm_documents(root.path());
        let team = realm("team");
        let credentials = root.path().join("credentials");
        let store = Arc::new(CoverageStore {
            inner: FileTokenStore::new(&credentials),
            fences: vec![
                meerkat_core::AdmissionFence::realm(&root.path().join("realms"), &team).unwrap(),
                meerkat_core::AdmissionFence::credentials(&credentials, &team).unwrap(),
            ],
            writes: std::sync::Mutex::new(Vec::new()),
        });
        let persistence = ProviderAuthPersistence::new(
            Arc::clone(&store) as Arc<dyn TokenStore>,
            Arc::new(FileLockCoordinator::new(credentials.join("refresh-locks"))),
        );

        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &team,
            "desktop",
            secret_client(),
            ConnectorClientSecretUpdate::Set("secret-one".into()),
        )
        .await
        .expect("a put with no holder succeeds");
        let after_put = document_generation(&documents, &team);
        assert!(after_put > 0, "the put moved the document's generation");
        assert_eq!(
            stored_client_secret(&documents, store.as_ref(), &team, "desktop").await,
            Some("secret-one".to_owned())
        );

        rotate_connector_client_secret(
            Arc::clone(&documents),
            &persistence,
            &team,
            "desktop",
            "secret-two",
        )
        .await
        .expect("a rotation with no holder succeeds");
        assert_eq!(
            document_generation(&documents, &team),
            after_put,
            "a rotation writes no document"
        );
        assert_eq!(
            stored_client_secret(&documents, store.as_ref(), &team, "desktop").await,
            Some("secret-two".to_owned())
        );

        remove_connector_client(Arc::clone(&documents), &persistence, &team, "desktop")
            .await
            .expect("a removal with no holder succeeds");
        assert_eq!(
            document_generation(&documents, &team),
            after_put + 1,
            "the removal moved the document's generation"
        );
        assert!(
            stored_document(&documents, &team)
                .realm
                .get("team")
                .is_none_or(|section| section.connector_client.is_empty()),
            "the client is gone from the document"
        );
        assert!(
            store.inner.list().await.unwrap().is_empty(),
            "the client's secret is gone"
        );
        let writes = store
            .writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            writes,
            vec![true, true, true],
            "the put's save, the rotation's save and the removal's clear each ran under the \
             one guard that covers the document's fence and the credential fence"
        );
    }

    /// While an admission holds the realm document's fence, a put, a
    /// rotation and a removal are refused before anything is written: no
    /// secret is stored, replaced or cleared and the document is unchanged.
    /// A held credential fence refuses a put the same way. Once the holder
    /// releases, the put lands.
    #[tokio::test]
    async fn under_a_held_fence_put_rotate_and_remove_refuse_not_started_with_nothing_written() {
        use meerkat_providers::auth_store::{
            FileLockCoordinator, FileTokenStore, ProviderAuthPersistence, TokenStore,
        };
        let root = tempfile::tempdir().unwrap();
        let documents = realm_documents(root.path());
        let team = realm("team");
        let credentials = root.path().join("credentials");
        let secrets = Arc::new(FileTokenStore::new(&credentials));
        let persistence = ProviderAuthPersistence::new(
            Arc::clone(&secrets) as Arc<dyn TokenStore>,
            Arc::new(FileLockCoordinator::new(credentials.join("refresh-locks"))),
        );
        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &team,
            "desktop",
            secret_client(),
            ConnectorClientSecretUpdate::Set("secret-one".into()),
        )
        .await
        .expect("a put with no holder succeeds");
        let document = documents.config_doc_path(&team);
        let before = std::fs::read(&document).unwrap();
        let generation = document_generation(&documents, &team);

        let document_fence =
            meerkat_core::AdmissionFence::realm(&root.path().join("realms"), &team).unwrap();
        let mut hold = meerkat_core::AdmissionHold::try_acquire(vec![document_fence])
            .expect("an admission holds the realm document's fence");
        let retargeted = ConnectorClientConfig {
            client: "replacement-client".into(),
            ..secret_client()
        };
        let put = put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &team,
            "desktop",
            retargeted.clone(),
            ConnectorClientSecretUpdate::Set("secret-two".into()),
        )
        .await;
        assert!(
            matches!(put, Err(ConnectorClientUpdateError::Ownership(_))),
            "a put under a held document fence did not start: {put:?}"
        );
        let rotation = rotate_connector_client_secret(
            Arc::clone(&documents),
            &persistence,
            &team,
            "desktop",
            "secret-three",
        )
        .await;
        assert!(
            matches!(rotation, Err(ConnectorClientUpdateError::Ownership(_))),
            "a rotation under a held document fence did not start: {rotation:?}"
        );
        let removal =
            remove_connector_client(Arc::clone(&documents), &persistence, &team, "desktop").await;
        assert!(
            matches!(removal, Err(ConnectorClientUpdateError::Ownership(_))),
            "a removal under a held document fence did not start: {removal:?}"
        );
        assert_eq!(
            std::fs::read(&document).unwrap(),
            before,
            "the document is unchanged"
        );
        assert_eq!(document_generation(&documents, &team), generation);
        assert_eq!(
            stored_client_secret(&documents, secrets.as_ref(), &team, "desktop").await,
            Some("secret-one".to_owned()),
            "the stored secret is unchanged"
        );
        assert_eq!(
            secrets.list().await.unwrap().len(),
            1,
            "no secret was added"
        );
        hold.release();

        let credential_fence =
            meerkat_core::AdmissionFence::credentials(&credentials, &team).unwrap();
        let mut hold = meerkat_core::AdmissionHold::try_acquire(vec![credential_fence])
            .expect("an admission holds the realm's credential fence");
        let put = put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &team,
            "desktop",
            retargeted.clone(),
            ConnectorClientSecretUpdate::Set("secret-two".into()),
        )
        .await;
        assert!(
            matches!(put, Err(ConnectorClientUpdateError::Ownership(_))),
            "a put under a held credential fence did not start: {put:?}"
        );
        assert_eq!(
            std::fs::read(&document).unwrap(),
            before,
            "the document is unchanged"
        );
        assert_eq!(
            secrets.list().await.unwrap().len(),
            1,
            "no secret was added"
        );
        hold.release();

        put_connector_client(
            Arc::clone(&documents),
            &persistence,
            &team,
            "desktop",
            retargeted,
            ConnectorClientSecretUpdate::Set("secret-two".into()),
        )
        .await
        .expect("the put lands once the holders released");
        assert_eq!(
            stored_client_secret(&documents, secrets.as_ref(), &team, "desktop").await,
            Some("secret-two".to_owned())
        );
        assert_eq!(
            secrets.list().await.unwrap().len(),
            1,
            "the retargeted client's secret replaced the old one"
        );
    }

    /// A persisted credential store whose first save waits until released.
    struct HeldSaveStore {
        inner: meerkat_providers::auth_store::FileTokenStore,
        entered: tokio::sync::Notify,
        release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[async_trait::async_trait]
    impl meerkat_providers::auth_store::TokenStore for HeldSaveStore {
        async fn load(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.load(key).await
        }
        async fn save(
            &self,
            guard: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
            tokens: &meerkat_core::auth::PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            let held = self.release.lock().await.take();
            if let Some(release) = held {
                self.entered.notify_one();
                let _ = release.await;
            }
            self.inner.save(guard, key, tokens).await
        }
        async fn clear(
            &self,
            guard: &meerkat_core::AdmissionGuard,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.inner.clear(guard, key).await
        }
        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.list().await
        }
        fn backend_name(&self) -> &'static str {
            "held-save"
        }
    }

    /// A caller that stops waiting once the update has entered does not
    /// stop it: the update settles (its secret and its document entry both
    /// land), and its fences are released only after that last write.
    #[tokio::test]
    async fn a_caller_that_stops_waiting_after_entry_still_settles_then_releases() {
        use meerkat_providers::auth_store::{
            FileLockCoordinator, FileTokenStore, ProviderAuthPersistence, TokenStore,
        };
        let root = tempfile::tempdir().unwrap();
        let documents = realm_documents(root.path());
        let team = realm("team");
        let credentials = root.path().join("credentials");
        let (release, held) = tokio::sync::oneshot::channel();
        let store = Arc::new(HeldSaveStore {
            inner: FileTokenStore::new(&credentials),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Mutex::new(Some(held)),
        });
        let persistence = ProviderAuthPersistence::new(
            Arc::clone(&store) as Arc<dyn TokenStore>,
            Arc::new(FileLockCoordinator::new(credentials.join("refresh-locks"))),
        );
        let caller = tokio::spawn({
            let (documents, persistence, team) =
                (Arc::clone(&documents), persistence.clone(), team.clone());
            async move {
                put_connector_client(
                    documents,
                    &persistence,
                    &team,
                    "desktop",
                    secret_client(),
                    ConnectorClientSecretUpdate::Set("secret-one".into()),
                )
                .await
            }
        });
        store.entered.notified().await;
        caller.abort();
        assert!(
            caller.await.is_err_and(|error| error.is_cancelled()),
            "the caller stopped waiting"
        );
        let document_fence =
            meerkat_core::AdmissionFence::realm(&root.path().join("realms"), &team).unwrap();
        assert!(
            matches!(
                meerkat_core::AdmissionGuard::try_acquire(vec![document_fence.clone()]),
                Err(meerkat_core::AdmissionRefusal::InProgress)
            ),
            "the entered update still holds the realm document's fence"
        );
        assert!(release.send(()).is_ok(), "the held save is released");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match meerkat_core::AdmissionGuard::try_acquire(vec![document_fence.clone()]) {
                Ok(guard) => {
                    drop(guard);
                    break;
                }
                Err(meerkat_core::AdmissionRefusal::InProgress)
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Err(refusal) => panic!("the fence was never released: {refusal}"),
            }
        }
        // The fences are free only now, and every write of the update has
        // already landed.
        assert_eq!(
            stored_client_secret(&documents, store.as_ref(), &team, "desktop").await,
            Some("secret-one".to_owned()),
            "the secret and the document entry landed before the release"
        );
    }
}
