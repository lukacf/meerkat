//! Host-registered (fixed) OAuth clients for MCP servers.
//!
//! An authorization server that offers no dynamic client registration signs
//! in only clients registered with it ahead of time: a client id, the exact
//! redirect registered for it and a client secret the client authenticates
//! with at the token endpoint. A host installs such a client for one MCP
//! credential slot (the server as its realm configuration defines it, in
//! the realm the host serves, with its account selection):
//! [`McpOAuthAuthority::install_client_profile_entered`].
//!
//! The profile is the slot's own client configuration. Its non-secret facts
//! and its secret are kept together in the credential store, under a key
//! kind of their own ([`TokenKey::mcp_oauth_client`]) derived from the slot,
//! so a profile is never read for another server, realm or account
//! selection. Its canonical non-secret meaning (the slot, issuer, client id,
//! redirect, authentication method, scopes and resource) is bound into every
//! attempt admitted under it and into every credential minted under it;
//! the secret is not part of the meaning, so rotating it keeps both.
//!
//! With a profile installed, a login neither needs nor uses dynamic
//! registration: it authorizes as the profile's client on its registered
//! redirect (PKCE S256 as always), and its code exchange and every refresh of
//! the resulting credential authenticate the client by the profile's method.
//! A credential minted under a profile refreshes only under that exact
//! profile; with the profile removed or replaced, its refresh closes the
//! credential as needing a new login and is never sent as a public client.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    McpOAuthAuthority, McpOAuthError, McpRegisteredRedirectBase, McpServerIdentity,
    StoredMcpOAuthClient, is_loopback_url, map_coordinated_login_error, mcp_store_error,
    registered_redirect_fits, valid_registered_redirect,
};
use crate::auth_oauth::OAuthClientAuthentication;
use crate::auth_store::{
    CredentialMutationError, CredentialMutationFn, CredentialMutationOutcome, PersistedAuthMode,
    PersistedTokens, TokenKey, TokenStore, TokenStoreAccess,
};
use meerkat_core::{
    AdministeredChange, AdministrationEntry, AdministrationEntryRefusal, AdministrationOutcome,
    EnteredAdministration,
};

/// The metadata member of a profile's custody row that holds its
/// non-secret facts; the secret is the row's primary secret.
const CLIENT_PROFILE_FIELD: &str = "mcp_oauth_client";

/// Longest value a profile field may hold.
const MAX_FIELD_LEN: usize = 4096;

/// Most scopes a profile may request.
const MAX_SCOPES: usize = 256;

/// The form of every client profile meaning: this prefix, then the SHA-256
/// of the profile's non-secret facts in lowercase hex.
const CLIENT_PROFILE_BINDING_PREFIX: &str = "mcp-client@";

/// Whether `value` has the form of a client profile meaning
/// ([`ClientProfileRecord::binding`]).
pub(super) fn is_client_profile_binding(value: &str) -> bool {
    value
        .strip_prefix(CLIENT_PROFILE_BINDING_PREFIX)
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// How a host-registered MCP OAuth client authenticates at the token
/// endpoint (RFC 6749 section 2.3.1; the `token_endpoint_auth_method` names
/// of RFC 7591).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpOAuthClientAuthMethod {
    /// HTTP Basic authentication with the client id and secret.
    ClientSecretBasic,
    /// `client_id` and `client_secret` in the token request body.
    ClientSecretPost,
}

impl McpOAuthClientAuthMethod {
    /// The method's RFC 7591 `token_endpoint_auth_method` name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientSecretBasic => "client_secret_basic",
            Self::ClientSecretPost => "client_secret_post",
        }
    }
}

impl std::fmt::Display for McpOAuthClientAuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The secret of a host-registered MCP OAuth client, on its way into the
/// credential store, or out of it for one token request. It has no
/// `Display`, `Clone` or serialization, its `Debug` is redacted, and its own
/// buffer is overwritten when it is dropped (best effort: copies a store
/// backend or a request body made are not this value's).
pub struct McpOAuthClientSecret(String);

impl McpOAuthClientSecret {
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// The secret, for the write that stores it or the request it
    /// authenticates.
    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for McpOAuthClientSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpOAuthClientSecret(<redacted>)")
    }
}

impl Drop for McpOAuthClientSecret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        // Keep the overwrite: the buffer is read before it is freed.
        std::hint::black_box(&bytes);
    }
}

/// A host-registered (pre-registered, confidential) OAuth client for one MCP
/// credential slot, for an authorization server that offers no dynamic
/// client registration. Install it with
/// [`McpOAuthAuthority::install_client_profile_entered`].
///
/// - `issuer`: the authorization server the client is registered with. The
///   server's protected resource metadata must name it. Under an `https`
///   issuer the token endpoint, where the secret is sent, must use `https`
///   too; only an `http` loopback issuer keeps loopback endpoints.
/// - `client_id` and `redirect_uri`: the registered client and its exact
///   registered redirect: the host's registered `https` redirect base itself
///   (one central callback for every slot of the client, told apart by
///   state), the target's own registered `https` callback
///   ([`McpOAuthAuthority::registered_redirect_uri`]), or an `http` loopback
///   redirect on `127.0.0.1` or `localhost` with an explicit port. A login
///   uses exactly this redirect, byte for byte.
/// - `auth_method` and the secret: how the code exchange and every refresh
///   authenticate the client.
/// - [`Self::with_scopes`]: the scopes the client requests, in place of the
///   ones the server's challenge or resource metadata names (for a client
///   registered for fewer scopes than the server lists).
/// - [`Self::with_resource`]: the protected resource identifier the server's
///   resource metadata names, when it differs from the configured server URL
///   (for example a server whose metadata names its origin for an endpoint
///   under a path). It must be the server URL's origin or a path above it.
///
/// `Debug` never shows the secret.
pub struct McpOAuthClientProfile {
    issuer: String,
    client_id: String,
    redirect_uri: String,
    auth_method: McpOAuthClientAuthMethod,
    secret: McpOAuthClientSecret,
    scopes: Option<BTreeSet<String>>,
    resource: Option<String>,
}

impl McpOAuthClientProfile {
    pub fn new(
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        redirect_uri: impl Into<String>,
        auth_method: McpOAuthClientAuthMethod,
        secret: McpOAuthClientSecret,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
            redirect_uri: redirect_uri.into(),
            auth_method,
            secret,
            scopes: None,
            resource: None,
        }
    }

    /// Request `scopes` and require them in the grant, in place of the
    /// scopes the server names. `offline_access` is only requested, never
    /// required, as for every login.
    #[must_use]
    pub fn with_scopes(mut self, scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.scopes = Some(scopes.into_iter().map(Into::into).collect());
        self
    }

    /// Accept `resource` as the protected resource identifier the server's
    /// resource metadata names, in place of the configured server URL.
    #[must_use]
    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn auth_method(&self) -> McpOAuthClientAuthMethod {
        self.auth_method
    }

    pub fn scopes(&self) -> Option<&BTreeSet<String>> {
        self.scopes.as_ref()
    }

    pub fn resource(&self) -> Option<&str> {
        self.resource.as_deref()
    }
}

impl std::fmt::Debug for McpOAuthClientProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthClientProfile")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("auth_method", &self.auth_method)
            .field("secret", &self.secret)
            .field("scopes", &self.scopes)
            .field("resource", &self.resource)
            .finish()
    }
}

/// The secret-free facts of the client profile installed for an MCP
/// credential slot ([`McpOAuthAuthority::client_profile`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpOAuthInstalledClientProfile {
    pub issuer: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub auth_method: McpOAuthClientAuthMethod,
    pub scopes: Option<BTreeSet<String>>,
    pub resource: Option<String>,
}

/// Why an MCP credential slot's client profile refused an operation
/// ([`McpOAuthError::ClientProfile`]). No variant carries the secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum McpOAuthClientProfileRefusal {
    /// The profile cannot serve the slot; the reason names the field.
    /// Nothing was written.
    #[error("the client profile is invalid: {0}")]
    Invalid(&'static str),
    /// An install found a profile already installed for the slot. Nothing
    /// was written; replace or remove it.
    #[error("a client profile is already installed for this server")]
    AlreadyInstalled,
    /// A replace found no profile installed for the slot. Nothing was
    /// written.
    #[error("no client profile is installed for this server")]
    NotInstalled,
    /// The host's entry check refused the update at its entry. Nothing was
    /// written.
    #[error(transparent)]
    NotAdmitted(AdministrationEntryRefusal),
    /// The login's redirect is not the profile's registered redirect.
    /// Nothing was admitted.
    #[error("the login's redirect is not the client profile's registered redirect")]
    RedirectMismatch,
    /// The server's protected resource metadata does not name the profile's
    /// issuer as an authorization server. Nothing was admitted.
    #[error("the server's authorization server is not the client profile's issuer")]
    IssuerMismatch,
    /// The server's protected resource metadata names another resource than
    /// the one the profile accepts. Nothing was admitted.
    #[error("the server's protected resource is not the one the client profile accepts")]
    ResourceMismatch,
    /// The custody row at the slot's profile key is not a profile installed
    /// for this slot (another server, realm or account selection, or no
    /// valid profile). It is refused, never ignored; remove it to clear it.
    #[error("the stored client profile belongs to another server, realm or account selection")]
    BindingMismatch,
    /// The attempt was admitted under a profile that has since been removed
    /// or replaced. Its attempt is retired; start a new login.
    #[error("the client profile this login was started under was removed or replaced")]
    Changed,
}

/// A profile's non-secret facts as its custody row stores them, under
/// [`CLIENT_PROFILE_FIELD`], with the slot it is installed for.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientProfileRecord {
    /// The slot's credential binding (server name and URL, realm and
    /// account selection).
    slot: String,
    server_name: String,
    server_url: String,
    issuer: String,
    client_id: String,
    redirect_uri: String,
    auth_method: McpOAuthClientAuthMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scopes: Option<BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
}

impl ClientProfileRecord {
    /// The record of `profile` installed for `target`'s slot, after the
    /// profile is validated for it.
    fn for_target(
        target: &McpServerIdentity,
        profile: &McpOAuthClientProfile,
    ) -> Result<Self, McpOAuthClientProfileRefusal> {
        if profile.secret.expose().is_empty()
            || profile.secret.expose().len() > MAX_FIELD_LEN
            || profile.secret.expose().chars().any(char::is_control)
        {
            return Err(McpOAuthClientProfileRefusal::Invalid("client secret"));
        }
        let record = Self {
            slot: target.binding_slug(),
            server_name: target.server_name().to_owned(),
            server_url: target.server_url().to_owned(),
            issuer: profile.issuer.clone(),
            client_id: profile.client_id.clone(),
            redirect_uri: profile.redirect_uri.clone(),
            auth_method: profile.auth_method,
            scopes: profile.scopes.clone(),
            resource: profile.resource.clone(),
        };
        record.validate()?;
        Ok(record)
    }

    /// Refuse facts no login of the slot could use.
    fn validate(&self) -> Result<(), McpOAuthClientProfileRefusal> {
        use McpOAuthClientProfileRefusal::Invalid;
        if !valid_endpoint_url(&self.issuer) {
            return Err(Invalid("issuer"));
        }
        if !valid_atom(&self.client_id) {
            return Err(Invalid("client id"));
        }
        if !valid_registered_redirect(&self.redirect_uri)
            && loopback_redirect_parts(&self.redirect_uri).is_none()
        {
            return Err(Invalid("redirect URI"));
        }
        if let Some(scopes) = &self.scopes
            && (scopes.is_empty()
                || scopes.len() > MAX_SCOPES
                || !scopes.iter().all(|scope| valid_scope_token(scope)))
        {
            return Err(Invalid("scopes"));
        }
        if let Some(resource) = &self.resource
            && !resource_covers_server(resource, &self.server_url)
        {
            return Err(Invalid("resource"));
        }
        Ok(())
    }

    /// Whether this record was installed for exactly `target`'s slot.
    fn matches_slot(&self, target: &McpServerIdentity) -> bool {
        self.slot == target.binding_slug()
            && self.server_name == target.server_name()
            && self.server_url == target.server_url()
    }

    /// The profile's canonical non-secret meaning: bound into the attempts
    /// and credentials minted under it. Rotating the secret keeps it.
    fn binding(&self) -> String {
        let mut hash = Sha256::new();
        let mut field = |value: &str| {
            hash.update(value.len().to_string().as_bytes());
            hash.update(b":");
            hash.update(value.as_bytes());
        };
        field("mcp-oauth-client-v1");
        field(&self.slot);
        field(&self.server_name);
        field(&self.server_url);
        field(&self.issuer);
        field(&self.client_id);
        field(&self.redirect_uri);
        field(self.auth_method.as_str());
        // Optional sections are tagged, and the scope list is counted, so no
        // list encodes like another field.
        if let Some(scopes) = &self.scopes {
            field("#scopes");
            field(&scopes.len().to_string());
            for scope in scopes {
                field(scope);
            }
        }
        if let Some(resource) = &self.resource {
            field("#resource");
            field(resource);
        }
        format!("{CLIENT_PROFILE_BINDING_PREFIX}{:x}", hash.finalize())
    }
}

/// The client profile installed for one slot, read from custody with its
/// secret, for one login or refresh.
pub(super) struct BoundClientProfile {
    record: ClientProfileRecord,
    secret: McpOAuthClientSecret,
    binding: String,
}

impl BoundClientProfile {
    /// The profile's canonical meaning (see [`ClientProfileRecord::binding`]).
    pub(super) fn binding(&self) -> &str {
        &self.binding
    }

    pub(super) fn issuer(&self) -> &str {
        &self.record.issuer
    }

    pub(super) fn client_id(&self) -> &str {
        &self.record.client_id
    }

    pub(super) fn redirect_uri(&self) -> &str {
        &self.record.redirect_uri
    }

    pub(super) fn scopes(&self) -> Option<&BTreeSet<String>> {
        self.record.scopes.as_ref()
    }

    /// The protected resource identifier the server's resource metadata
    /// must name: the profile's, or the configured server URL.
    pub(super) fn expected_resource(&self) -> &str {
        self.record
            .resource
            .as_deref()
            .unwrap_or(&self.record.server_url)
    }

    /// How a token request authenticates this client.
    pub(super) fn client_authentication(&self) -> OAuthClientAuthentication<'_> {
        match self.record.auth_method {
            McpOAuthClientAuthMethod::ClientSecretBasic => {
                OAuthClientAuthentication::SecretBasic(self.secret.expose())
            }
            McpOAuthClientAuthMethod::ClientSecretPost => {
                OAuthClientAuthentication::SecretPost(self.secret.expose())
            }
        }
    }

    /// The client a credential minted under this profile is stored with:
    /// its id, method, registered redirect and meaning, never its secret.
    pub(super) fn stored_client(&self) -> StoredMcpOAuthClient {
        StoredMcpOAuthClient {
            client_id: self.record.client_id.clone(),
            client_secret: None,
            token_endpoint_auth_method: self.record.auth_method.as_str().to_owned(),
            redirect_uri: self.record.redirect_uri.clone(),
            client_profile: Some(self.binding.clone()),
        }
    }

    /// The loopback listener a host binds for this profile's redirect: its
    /// host, port and path. `None` for a registered `https` redirect.
    pub(super) fn loopback_bind(&self) -> Option<(&str, u16, &str)> {
        loopback_redirect_parts(&self.record.redirect_uri)
    }

    fn installed(&self) -> McpOAuthInstalledClientProfile {
        McpOAuthInstalledClientProfile {
            issuer: self.record.issuer.clone(),
            client_id: self.record.client_id.clone(),
            redirect_uri: self.record.redirect_uri.clone(),
            auth_method: self.record.auth_method,
            scopes: self.record.scopes.clone(),
            resource: self.record.resource.clone(),
        }
    }
}

/// The custody key of `target`'s client profile: derived from the slot's
/// own credential binding.
fn client_profile_key(target: &McpServerIdentity) -> Result<TokenKey, McpOAuthError> {
    let slot = target.auth_binding_ref()?;
    Ok(TokenKey::mcp_oauth_client(slot.realm, slot.binding))
}

fn profile_refusal(
    target: &McpServerIdentity,
    refusal: McpOAuthClientProfileRefusal,
) -> McpOAuthError {
    McpOAuthError::ClientProfile {
        server_name: target.server_name().to_owned(),
        refusal,
    }
}

/// The profile a custody row holds for `target`'s slot. A row that is not
/// exactly a valid profile installed for this slot is refused.
fn bound_profile_from_row(
    target: &McpServerIdentity,
    row: PersistedTokens,
) -> Result<BoundClientProfile, McpOAuthError> {
    let mismatch = || profile_refusal(target, McpOAuthClientProfileRefusal::BindingMismatch);
    if row.auth_mode != PersistedAuthMode::McpOauthClient {
        return Err(mismatch());
    }
    let record: ClientProfileRecord = row
        .metadata
        .get(CLIENT_PROFILE_FIELD)
        .cloned()
        .and_then(|record| serde_json::from_value(record).ok())
        .ok_or_else(mismatch)?;
    if !record.matches_slot(target) || record.validate().is_err() {
        return Err(mismatch());
    }
    let secret = row
        .primary_secret
        .filter(|secret| !secret.is_empty())
        .map(McpOAuthClientSecret::new)
        .ok_or_else(mismatch)?;
    Ok(BoundClientProfile {
        binding: record.binding(),
        record,
        secret,
    })
}

/// The client profile installed for `target`'s slot, if any, read with
/// `access`. A row at the slot's profile key that is not a valid profile
/// for exactly this slot is refused
/// ([`McpOAuthClientProfileRefusal::BindingMismatch`]): the slot never
/// falls back to dynamic registration beside it.
pub(super) async fn load_client_profile(
    store: &dyn TokenStore,
    target: &McpServerIdentity,
    access: TokenStoreAccess,
) -> Result<Option<BoundClientProfile>, McpOAuthError> {
    let key = client_profile_key(target)?;
    store
        .load_for(&key, access)
        .await
        .map_err(mcp_store_error)?
        .map(|row| bound_profile_from_row(target, row))
        .transpose()
}

/// The custody row of `record` with `secret`.
fn client_profile_row(
    record: &ClientProfileRecord,
    secret: &McpOAuthClientSecret,
) -> Result<PersistedTokens, McpOAuthError> {
    let mut row = PersistedTokens::api_key(secret.expose());
    row.auth_mode = PersistedAuthMode::McpOauthClient;
    row.metadata = serde_json::json!({
        CLIENT_PROFILE_FIELD: serde_json::to_value(record).map_err(|error| {
            McpOAuthError::TokenStore(format!("failed to serialize the client profile: {error}"))
        })?,
    });
    Ok(row)
}

/// The validated record of `profile` for the scoped `target`. Only a target
/// with an account selection logs in, so only such a slot takes a profile.
fn profile_record(
    target: &McpServerIdentity,
    profile: &McpOAuthClientProfile,
    registered_base: Option<&McpRegisteredRedirectBase>,
) -> Result<ClientProfileRecord, McpOAuthError> {
    if !target.is_selected() {
        return Err(McpOAuthError::AccountSelectionRequired);
    }
    let record = ClientProfileRecord::for_target(target, profile)
        .map_err(|refusal| profile_refusal(target, refusal))?;
    // A registered `https` redirect is one of this host's registered
    // redirects for the slot: its base itself (the central callback) or the
    // slot's derived redirect. A host that serves no base has none.
    if loopback_redirect_parts(&record.redirect_uri).is_none()
        && !registered_base
            .is_some_and(|base| registered_redirect_fits(target, base, &record.redirect_uri))
    {
        return Err(profile_refusal(
            target,
            McpOAuthClientProfileRefusal::Invalid("redirect URI"),
        ));
    }
    Ok(record)
}

/// One profile update of a slot.
enum ProfileUpdate {
    /// Store a profile where none is installed.
    Install(ClientProfileRecord, McpOAuthClientSecret),
    /// Store a profile over the one installed.
    Replace(ClientProfileRecord, McpOAuthClientSecret),
    /// Remove whatever the slot's profile key holds.
    Remove,
}

/// The admission the host's entry check granted, held until the update has
/// settled.
type EnteredSlot = Arc<parking_lot::Mutex<Option<EnteredAdministration>>>;

impl McpOAuthAuthority {
    /// Install `profile` as the client of `target`'s credential slot (the
    /// target as this authority serves it: its realm and account selection
    /// included). From then on, a login of the slot uses the profile and
    /// never dynamic registration. Refused when a profile is already
    /// installed ([`McpOAuthClientProfileRefusal::AlreadyInstalled`]).
    ///
    /// `registered_base` is the host's registered redirect base, when it
    /// serves one. A profile whose redirect is a registered `https` redirect
    /// must name exactly that base (the central callback every slot of the
    /// client may share) or the slot's redirect derived from it
    /// ([`Self::registered_redirect_uri`]), byte for byte; any other is
    /// refused ([`McpOAuthClientProfileRefusal::Invalid`]).
    ///
    /// The update runs under the credential mutation owner's exclusion for
    /// the slot's profile key. `entry` admits the caller there, after the
    /// slot's current profile was read and immediately before anything is
    /// written; a refusal writes nothing
    /// ([`McpOAuthClientProfileRefusal::NotAdmitted`]). The admission is
    /// held until the write's durable result is known. An admission that
    /// states its administrator ([`EnteredAdministration::administered_by`])
    /// hands the host's recorder one record then, also when the caller
    /// stopped waiting: the slot's install, replace or removal, committed
    /// or unknown. A write that fails
    /// may have left the slot's profile in any state: read it back with
    /// [`Self::client_profile`]. The caller is the host's authenticated
    /// administrator: this method is not an authentication.
    pub async fn install_client_profile_entered(
        &self,
        target: &McpServerIdentity,
        profile: McpOAuthClientProfile,
        registered_base: Option<&McpRegisteredRedirectBase>,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<(), McpOAuthError> {
        let target = self.scoped(target)?;
        let record = profile_record(&target, &profile, registered_base)?;
        let McpOAuthClientProfile { secret, .. } = profile;
        self.update_client_profile(&target, ProfileUpdate::Install(record, secret), entry)
            .await
    }

    /// Replace the client profile installed for `target`'s credential slot
    /// with `profile`, as [`Self::install_client_profile_entered`] installs
    /// one (its redirect checked against `registered_base` the same way);
    /// refused when none is installed
    /// ([`McpOAuthClientProfileRefusal::NotInstalled`]).
    ///
    /// The same facts with a new secret rotate the secret: attempts and
    /// credentials bound to the profile keep it, and the next code exchange
    /// or refresh authenticates with the new secret. Changed facts are
    /// another profile: an attempt admitted under the previous one no longer
    /// completes, and a credential minted under it can no longer refresh
    /// (its next refresh closes it as needing a new login).
    pub async fn replace_client_profile_entered(
        &self,
        target: &McpServerIdentity,
        profile: McpOAuthClientProfile,
        registered_base: Option<&McpRegisteredRedirectBase>,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<(), McpOAuthError> {
        let target = self.scoped(target)?;
        let record = profile_record(&target, &profile, registered_base)?;
        let McpOAuthClientProfile { secret, .. } = profile;
        self.update_client_profile(&target, ProfileUpdate::Replace(record, secret), entry)
            .await
    }

    /// Remove the client profile of `target`'s credential slot (whatever its
    /// profile key holds); removing none succeeds. A later login of the slot
    /// uses dynamic registration again. A credential minted under the
    /// removed profile is not removed: it is used until it needs a refresh,
    /// which closes it as needing a new login. Admitted through `entry` as
    /// [`Self::install_client_profile_entered`] is.
    pub async fn remove_client_profile_entered(
        &self,
        target: &McpServerIdentity,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<(), McpOAuthError> {
        let target = self.scoped(target)?;
        self.update_client_profile(&target, ProfileUpdate::Remove, entry)
            .await
    }

    /// The secret-free facts of the client profile installed for
    /// `target`'s credential slot, if any. An unattended read of the
    /// credential store.
    pub async fn client_profile(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<McpOAuthInstalledClientProfile>, McpOAuthError> {
        let target = self.scoped(target)?;
        Ok(load_client_profile(
            self.token_store().as_ref(),
            &target,
            TokenStoreAccess::Unattended,
        )
        .await?
        .map(|profile| profile.installed()))
    }

    /// Run `update` for the scoped `target` under the credential mutation
    /// owner of the slot's profile key, admitting the caller through
    /// `entry` immediately before the write.
    async fn update_client_profile(
        &self,
        target: &McpServerIdentity,
        update: ProfileUpdate,
        entry: Arc<dyn AdministrationEntry>,
    ) -> Result<(), McpOAuthError> {
        let key = client_profile_key(target)?;
        let store = self.token_store();
        let outcome: Arc<parking_lot::Mutex<Option<Result<(), McpOAuthError>>>> = Arc::default();
        let entered: EnteredSlot = Arc::default();
        let (outcome_slot, entered_slot) = (Arc::clone(&outcome), Arc::clone(&entered));
        let (mutation_key, mutation_target) = (key.clone(), target.clone());
        let mutation: CredentialMutationFn = Box::new(move || {
            Box::pin(async move {
                let written = write_profile_update(
                    store.as_ref(),
                    &mutation_key,
                    &mutation_target,
                    update,
                    entry.as_ref(),
                    &entered_slot,
                )
                .await;
                let (result, durable) = match written {
                    Ok(durable) => (Ok(()), Ok(durable)),
                    Err(error) => {
                        let durable = Err(CredentialMutationError::Operation(error.to_string()));
                        (Err(error), durable)
                    }
                };
                *outcome_slot.lock() = Some(result);
                durable
            })
        });
        let admitted = self
            .refresh_coordinator()
            .with_exclusive_mutation_for(key, TokenStoreAccess::Interactive, mutation)
            .await;
        let result = outcome.lock().take();
        // The update has settled: only now is the entered admission released.
        drop(entered.lock().take());
        match (admitted, result) {
            (_, Some(result)) => result,
            // The update never ran: the owner refused or lost it.
            (Err(error), None) => Err(map_coordinated_login_error(target, error)),
            (Ok(_), None) => Err(McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_owned(),
                reason: "the credential mutation owner returned without the update's result"
                    .to_owned(),
            }),
        }
    }
}

/// Read the slot's current profile row, check `update` against it, let
/// `entry` admit the caller, then write. The admission goes to `entered`.
async fn write_profile_update(
    store: &dyn TokenStore,
    key: &TokenKey,
    target: &McpServerIdentity,
    update: ProfileUpdate,
    entry: &dyn AdministrationEntry,
    entered: &EnteredSlot,
) -> Result<CredentialMutationOutcome, McpOAuthError> {
    let current = store
        .load_for(key, TokenStoreAccess::Interactive)
        .await
        .map_err(mcp_store_error)?;
    match (&update, current) {
        (ProfileUpdate::Install(..), Some(_)) => {
            return Err(profile_refusal(
                target,
                McpOAuthClientProfileRefusal::AlreadyInstalled,
            ));
        }
        (ProfileUpdate::Replace(..), None) => {
            return Err(profile_refusal(
                target,
                McpOAuthClientProfileRefusal::NotInstalled,
            ));
        }
        // Only a profile installed for exactly this slot is replaced.
        (ProfileUpdate::Replace(..), Some(row)) => {
            bound_profile_from_row(target, row)?;
        }
        (ProfileUpdate::Install(..), None) | (ProfileUpdate::Remove, _) => {}
    }
    let change = administered_change(target, &update);
    let admission = entry
        .enter()
        .map_err(|refusal| {
            profile_refusal(target, McpOAuthClientProfileRefusal::NotAdmitted(refusal))
        })?
        .for_change(change);
    *entered.lock() = Some(admission);
    let written = async {
        match update {
            ProfileUpdate::Install(record, secret) | ProfileUpdate::Replace(record, secret) => {
                let row = client_profile_row(&record, &secret)?;
                store
                    .save_for(key, &row, TokenStoreAccess::Interactive)
                    .await
                    .map_err(mcp_store_error)?;
                Ok(CredentialMutationOutcome::Persisted(row))
            }
            ProfileUpdate::Remove => {
                store
                    .clear_for(key, TokenStoreAccess::Interactive)
                    .await
                    .map_err(mcp_store_error)?;
                Ok(CredentialMutationOutcome::Cleared)
            }
        }
    }
    .await;
    // The write's durable result is known: the admission's record says how
    // it ended, whoever releases the admission last.
    if let Some(admission) = entered.lock().as_mut() {
        admission.settle(if written.is_ok() {
            AdministrationOutcome::Committed
        } else {
            AdministrationOutcome::Unknown
        });
    }
    written
}

/// What `update` of `target`'s slot changes, as its admission's record
/// names it: the slot, never the client's secret.
fn administered_change(target: &McpServerIdentity, update: &ProfileUpdate) -> AdministeredChange {
    let (realm, server, server_url, slot) = (
        target.realm().cloned(),
        target.server_name().to_owned(),
        target.server_url().to_owned(),
        target.binding_slug(),
    );
    match update {
        ProfileUpdate::Install(..) => AdministeredChange::McpOauthClientProfileInstall {
            realm,
            server,
            server_url,
            slot,
        },
        ProfileUpdate::Replace(..) => AdministeredChange::McpOauthClientProfileReplace {
            realm,
            server,
            server_url,
            slot,
        },
        ProfileUpdate::Remove => AdministeredChange::McpOauthClientProfileRemove {
            realm,
            server,
            server_url,
            slot,
        },
    }
}

/// A profile atom: non-empty, bounded, without control characters.
fn valid_atom(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_FIELD_LEN && !value.chars().any(char::is_control)
}

/// An absolute `https` URL (or `http` on a loopback host) with a host and no
/// userinfo, query or fragment.
fn valid_endpoint_url(value: &str) -> bool {
    valid_atom(value)
        && reqwest::Url::parse(value).is_ok_and(|url| {
            (url.scheme() == "https" || (url.scheme() == "http" && is_loopback_url(&url)))
                && url.host_str().is_some_and(|host| !host.is_empty())
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
}

/// An RFC 6749 scope token: printable ASCII without space, `"` or `\`.
fn valid_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= MAX_FIELD_LEN
        && scope
            .chars()
            .all(|c| matches!(c, '\x21' | '\x23'..='\x5B' | '\x5D'..='\x7E'))
}

/// The host, port and path of a profile's loopback redirect: `http` on
/// `127.0.0.1` or `localhost`, an explicit non-zero port, a path of
/// unreserved characters, no userinfo, query or fragment, already in its
/// canonical form (so the redirect a host binds is exactly this one).
fn loopback_redirect_parts(redirect_uri: &str) -> Option<(&str, u16, &str)> {
    let url = reqwest::Url::parse(redirect_uri).ok()?;
    if url.scheme() != "http"
        || url.as_str() != redirect_uri
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = match url.host_str()? {
        "127.0.0.1" => "127.0.0.1",
        "localhost" => "localhost",
        _ => return None,
    };
    let port = url.port().filter(|port| *port != 0)?;
    // Canonical, without a query or fragment: the path ends the redirect.
    let path = redirect_uri.get(redirect_uri.len().checked_sub(url.path().len())?..)?;
    if path != url.path()
        || !path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '.' | '_' | '~'))
    {
        return None;
    }
    Some((host, port, path))
}

/// Whether `resource` names `server_url`'s own resource or one above it: the
/// same origin, and the server URL's path at or under the resource's path.
/// No userinfo, query or fragment.
pub(super) fn resource_covers_server(resource: &str, server_url: &str) -> bool {
    if !valid_endpoint_url(resource) {
        return false;
    }
    let (Ok(resource), Ok(server)) = (
        reqwest::Url::parse(resource),
        reqwest::Url::parse(server_url),
    ) else {
        return false;
    };
    if !crate::auth_oauth::same_origin(&resource, &server) {
        return false;
    }
    let resource_path = resource.path().trim_end_matches('/');
    let server_path = server.path().trim_end_matches('/');
    server_path == resource_path
        || server_path
            .strip_prefix(resource_path)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const SECRET: &str = "client-secret-canary-7f3a";

    fn target() -> McpServerIdentity {
        McpServerIdentity::from_server_config("remote", "https://mcp.example.test/mcp")
            .with_expected_account("subject-1")
            .unwrap()
    }

    fn profile(redirect: &str) -> McpOAuthClientProfile {
        McpOAuthClientProfile::new(
            "https://mcp.example.test",
            "registered-client",
            redirect,
            McpOAuthClientAuthMethod::ClientSecretPost,
            McpOAuthClientSecret::new(SECRET),
        )
    }

    #[test]
    fn debug_of_the_profile_and_its_secret_never_shows_the_secret() {
        let profile = profile("http://127.0.0.1:3118/callback").with_scopes(["read"]);
        let rendered = format!("{profile:?} {profile:#?} {:?}", profile.secret);
        assert!(!rendered.contains(SECRET), "{rendered}");
        assert!(rendered.contains("registered-client"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        let refusals = [
            McpOAuthClientProfileRefusal::Invalid("client secret"),
            McpOAuthClientProfileRefusal::Changed,
            McpOAuthClientProfileRefusal::BindingMismatch,
        ];
        for refusal in refusals {
            let error = McpOAuthError::ClientProfile {
                server_name: "remote".into(),
                refusal,
            };
            let rendered = format!("{error} {error:?}");
            assert!(!rendered.contains(SECRET), "{rendered}");
            assert!(error.is_refusal());
        }
    }

    #[test]
    fn a_profile_is_validated_for_its_slot() {
        let target = target();
        let accepted = |profile: McpOAuthClientProfile| {
            ClientProfileRecord::for_target(&target, &profile).is_ok()
        };
        assert!(accepted(profile("http://127.0.0.1:3118/callback")));
        assert!(accepted(profile(
            "http://localhost:3118/mcp/oauth/callback"
        )));
        assert!(accepted(profile(
            "https://host.example/mcp/oauth/callback/remote-AbC123dEf456GhI7"
        )));
        assert!(accepted(
            profile("http://127.0.0.1:3118/callback").with_resource("https://mcp.example.test")
        ));
        assert!(accepted(
            profile("http://127.0.0.1:3118/callback").with_resource("https://mcp.example.test/mcp")
        ));
        for redirect in [
            // No explicit port, a port of zero, or another loopback host.
            "http://127.0.0.1/callback",
            "http://127.0.0.1:0/callback",
            "http://[::1]:3118/callback",
            "http://host.example:3118/callback",
            // Query, fragment, userinfo or a non-canonical form.
            "http://127.0.0.1:3118/callback?x=1",
            "http://127.0.0.1:3118/callback#x",
            "http://user@127.0.0.1:3118/callback",
            "http://LOCALHOST:3118/callback",
            "http://127.0.0.1:3118/a/../callback",
            "http://127.0.0.1:3118/{param}",
            "https://host.example/callback?x=1",
            "",
        ] {
            assert!(!accepted(profile(redirect)), "{redirect:?} must be refused");
        }
        let mut no_secret = profile("http://127.0.0.1:3118/callback");
        no_secret.secret = McpOAuthClientSecret::new("");
        assert_eq!(
            ClientProfileRecord::for_target(&target, &no_secret).err(),
            Some(McpOAuthClientProfileRefusal::Invalid("client secret"))
        );
        let mut no_client = profile("http://127.0.0.1:3118/callback");
        no_client.client_id = String::new();
        assert!(!accepted(no_client));
        let mut remote_http_issuer = profile("http://127.0.0.1:3118/callback");
        remote_http_issuer.issuer = "http://mcp.example.test".into();
        assert!(!accepted(remote_http_issuer));
        for resource in [
            // Another origin, a sibling path, a path below the server's.
            "https://other.example.test",
            "https://mcp.example.test/other",
            "https://mcp.example.test/mcp/deeper",
            "http://mcp.example.test",
            "https://mcp.example.test/?x=1",
        ] {
            assert!(
                !accepted(profile("http://127.0.0.1:3118/callback").with_resource(resource)),
                "{resource:?} must be refused"
            );
        }
        for scopes in [vec![], vec!["two words"], vec!["quote\""]] {
            assert!(!accepted(
                profile("http://127.0.0.1:3118/callback").with_scopes(scopes)
            ));
        }
    }

    #[test]
    fn the_binding_covers_every_fact_and_the_slot_but_not_the_secret() {
        let target = target();
        let binding = |profile: McpOAuthClientProfile, target: &McpServerIdentity| {
            ClientProfileRecord::for_target(target, &profile)
                .unwrap()
                .binding()
        };
        let base = binding(profile("http://127.0.0.1:3118/callback"), &target);
        let mut rotated = profile("http://127.0.0.1:3118/callback");
        rotated.secret = McpOAuthClientSecret::new("another-secret");
        assert_eq!(binding(rotated, &target), base, "a rotated secret keeps it");
        let mut other_client = profile("http://127.0.0.1:3118/callback");
        other_client.client_id = "other-client".into();
        let mut other_issuer = profile("http://127.0.0.1:3118/callback");
        other_issuer.issuer = "https://issuer.example.test".into();
        let mut basic = profile("http://127.0.0.1:3118/callback");
        basic.auth_method = McpOAuthClientAuthMethod::ClientSecretBasic;
        for changed in [
            binding(other_client, &target),
            binding(other_issuer, &target),
            binding(basic, &target),
            binding(profile("http://127.0.0.1:3119/callback"), &target),
            binding(
                profile("http://127.0.0.1:3118/callback").with_scopes(["read"]),
                &target,
            ),
            binding(
                profile("http://127.0.0.1:3118/callback").with_resource("https://mcp.example.test"),
                &target,
            ),
            // The same profile for another slot: another realm, server or
            // account selection.
            binding(
                profile("http://127.0.0.1:3118/callback"),
                &target
                    .clone()
                    .in_realm(meerkat_core::connection::RealmId::parse("realm-b").unwrap()),
            ),
            binding(
                profile("http://127.0.0.1:3118/callback"),
                &McpServerIdentity::from_server_config("remote", "https://mcp.example.test/mcp")
                    .with_expected_account("subject-2")
                    .unwrap(),
            ),
        ] {
            assert_ne!(changed, base);
        }
    }

    #[test]
    fn a_row_for_another_slot_or_mode_is_refused_never_ignored() {
        let target = target();
        let record =
            ClientProfileRecord::for_target(&target, &profile("http://127.0.0.1:3118/callback"))
                .unwrap();
        let row = client_profile_row(&record, &McpOAuthClientSecret::new(SECRET)).unwrap();
        let bound = bound_profile_from_row(&target, row.clone()).unwrap();
        assert_eq!(bound.binding(), record.binding());
        assert!(is_client_profile_binding(bound.binding()));
        assert!(!is_client_profile_binding("desktop@0123"));
        assert!(!is_client_profile_binding("mcp-client@not-a-digest"));
        assert_eq!(
            bound.loopback_bind(),
            Some(("127.0.0.1", 3118, "/callback"))
        );
        let other_slot = target
            .clone()
            .in_realm(meerkat_core::connection::RealmId::parse("realm-b").unwrap());
        let mut other_mode = row.clone();
        other_mode.auth_mode = PersistedAuthMode::McpServiceSecret;
        let mut no_secret = row.clone();
        no_secret.primary_secret = None;
        for (target, row) in [
            (&other_slot, row.clone()),
            (&target, other_mode),
            (&target, no_secret),
        ] {
            assert!(matches!(
                bound_profile_from_row(target, row),
                Err(McpOAuthError::ClientProfile {
                    refusal: McpOAuthClientProfileRefusal::BindingMismatch,
                    ..
                })
            ));
        }
        // The stored row carries the secret only as its redacted primary
        // secret, never in its facts.
        assert!(!row.metadata.to_string().contains(SECRET));
        assert!(!format!("{row:?}").contains(SECRET));
    }
}
