//! The credential sources of personal servers.
//!
//! Two sources keep a personal server's credential its owner's alone:
//!
//! - [`PersonalMcpAccounts::member_credential_source`] wraps the credential
//!   source a composition gives its member factories. For a personal
//!   server's target it holds nothing: the stored bearer is absent and an
//!   interactive login needs the person, so a connection to the server from
//!   a member without its owner's source awaits a sign-in instead of
//!   authenticating as the system account. Every other target, and every
//!   stdio service secret, is the wrapped source's.
//! - The owner's source, which a member private to its owner carries in its
//!   build ([`PersonalAccountCredentials`]): a personal server's target is
//!   read in the owner's own credential realm, below the installation's
//!   realm (one token slot and one lifecycle lease per owner and server);
//!   every other target is the member factory's source's.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat::{McpOAuthError, McpServerIdentity};
use meerkat_core::RealmId;
use meerkat_mcp::McpAuthResolver;

use super::{McpAccountsConfig, PersonalMcpAccounts};

/// How many hexadecimal digits of a private-owner key name its owner's
/// credential realm.
const OWNER_REALM_DIGITS: usize = 32;

/// Where a composition keeps its people's personal accounts, and the
/// credential source its member factories use for every other server.
#[derive(Clone)]
pub struct PersonalAccountCredentials {
    /// The composition's account facade: its provider-auth persistence and
    /// its runtime's credential authority.
    host: meerkat::HostAuthService,
    /// The installation's realm, below which each owner's realm is named.
    installation_realm: RealmId,
    /// The member factories' own credential source for every other server;
    /// `None` when they connect without OAuth.
    system: Option<Arc<dyn McpAuthResolver>>,
}

impl PersonalAccountCredentials {
    /// Keep personal accounts in `host`'s provider-auth persistence, each
    /// owner's in a realm of their own below `installation_realm`, and read
    /// every other server's credential through `system`, the member
    /// factories' own source (the one
    /// [`PersonalMcpAccounts::member_credential_source`] wraps).
    pub fn new(
        host: meerkat::HostAuthService,
        installation_realm: RealmId,
        system: Option<Arc<dyn McpAuthResolver>>,
    ) -> Self {
        Self {
            host,
            installation_realm,
            system,
        }
    }

    /// The credential realm of the owner whose private-owner key is
    /// `owner_key`: `<installation realm>.personal.<key digits>`. `None` for
    /// a key that is not a private-owner key.
    pub(crate) fn owner_realm(&self, owner_key: &str) -> Option<RealmId> {
        let _ = (owner_key, OWNER_REALM_DIGITS, &self.installation_realm);
        None
    }

    /// The credential source of a member private to the owner whose
    /// private-owner key is `owner_key`. `None` when the owner's credential
    /// owner cannot be built.
    pub(crate) fn owner_source(
        &self,
        config: Arc<McpAccountsConfig>,
        owner_key: &str,
    ) -> Option<Arc<dyn McpAuthResolver>> {
        let owner_realm = self.owner_realm(owner_key)?;
        let owner = match self
            .host
            .clone()
            .with_mcp_realm(owner_realm.clone())
            .mcp_oauth_authority()
        {
            Ok(owner) => owner,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "the owner's MCP credential owner is unavailable; the member gets no \
                     personal server"
                );
                return None;
            }
        };
        Some(Arc::new(OwnerCredentialSource {
            config,
            owner: Arc::new(owner),
            owner_realm,
            system: self.system.clone(),
        }))
    }
}

/// A member factory's credential source, serving no personal server's
/// target (see the module docs).
struct NoPersonalFallback {
    inner: Arc<dyn McpAuthResolver>,
    config: Arc<McpAccountsConfig>,
}

impl PersonalMcpAccounts {
    /// `inner`, a member factory's MCP credential source, serving no
    /// personal server's target (see the module docs).
    pub fn member_credential_source(
        &self,
        inner: Arc<dyn McpAuthResolver>,
    ) -> Arc<dyn McpAuthResolver> {
        Arc::new(NoPersonalFallback {
            inner,
            config: Arc::clone(&self.config),
        })
    }
}

fn awaiting_owner(target: &McpServerIdentity) -> McpOAuthError {
    McpOAuthError::HumanAuthorizationRequired {
        server_name: target.server_name().to_owned(),
    }
}

#[async_trait]
impl McpAuthResolver for NoPersonalFallback {
    async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        let _ = (&self.config, awaiting_owner);
        self.inner.stored_bearer_token(target).await
    }

    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError> {
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
        server: &meerkat_core::McpServerConfig,
    ) -> Result<
        meerkat_core::auth::McpServiceSecretValues,
        meerkat_core::auth::McpServiceSecretRefusal,
    > {
        self.inner.stdio_service_secrets(server).await
    }

    fn selected_account(&self, target: &McpServerIdentity) -> meerkat_core::OperationAccount {
        if self.config.is_personal(target.server_name()) {
            return meerkat_core::OperationAccount::Unavailable;
        }
        self.inner.selected_account(target)
    }
}

/// The credential source of a member private to its owner (see the module
/// docs).
struct OwnerCredentialSource {
    config: Arc<McpAccountsConfig>,
    /// The owner's credential owner, scoped to `owner_realm`.
    owner: Arc<dyn McpAuthResolver>,
    owner_realm: RealmId,
    system: Option<Arc<dyn McpAuthResolver>>,
}

impl OwnerCredentialSource {
    /// `target` in the owner's credential realm, when it is a personal
    /// server's.
    fn owners(&self, target: &McpServerIdentity) -> Option<McpServerIdentity> {
        self.config
            .is_personal(target.server_name())
            .then(|| target.clone().in_realm(self.owner_realm.clone()))
    }
}

#[async_trait]
impl McpAuthResolver for OwnerCredentialSource {
    async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        if let Some(owners) = self.owners(target) {
            return self.owner.stored_bearer_token(&owners).await;
        }
        match &self.system {
            Some(system) => system.stored_bearer_token(target).await,
            None => Ok(None),
        }
    }

    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        if let Some(owners) = self.owners(target) {
            return self
                .owner
                .interactive_login(&owners, www_authenticate)
                .await;
        }
        match &self.system {
            Some(system) => system.interactive_login(target, www_authenticate).await,
            None => Err(awaiting_owner(target)),
        }
    }

    fn credential_realm(&self) -> Option<RealmId> {
        self.system
            .as_ref()
            .and_then(|system| system.credential_realm())
    }

    fn http_egress(&self) -> Option<meerkat_mcp::HttpEgressProfile> {
        match &self.system {
            Some(system) => system.http_egress(),
            None => self.owner.http_egress(),
        }
    }

    async fn stdio_service_secrets(
        &self,
        server: &meerkat_core::McpServerConfig,
    ) -> Result<
        meerkat_core::auth::McpServiceSecretValues,
        meerkat_core::auth::McpServiceSecretRefusal,
    > {
        match &self.system {
            Some(system) => system.stdio_service_secrets(server).await,
            None => Err(meerkat_core::auth::McpServiceSecretRefusal::NoCustody),
        }
    }

    fn selected_account(&self, target: &McpServerIdentity) -> meerkat_core::OperationAccount {
        if let Some(owners) = self.owners(target) {
            return self.owner.selected_account(&owners);
        }
        match &self.system {
            Some(system) => system.selected_account(target),
            None => meerkat_core::OperationAccount::Unavailable,
        }
    }
}
