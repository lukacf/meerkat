//! Immutable review evidence for an existing own-definition connector.
//! Native configuration and credential owners remain authoritative.
use meerkat::{ConnectorAuthStatus, ConnectorOAuthTarget};
use meerkat_auth_core::connector_oauth::AccountSelection;
use meerkat_core::{CredentialAccountId, CredentialAccountRef, McpServerConfig, ResolvedStorage};
use std::collections::BTreeSet;

#[derive(Clone)]
pub(crate) struct ExistingConnector {
    storage: ResolvedStorage,
    config: McpServerConfig,
    generation: u64,
    slot: CredentialAccountRef,
}

impl ExistingConnector {
    pub(crate) async fn select(storage: &ResolvedStorage, service: &str) -> Result<Self, &'static str> {
        let (servers, generation) = crate::realm::read_own_mcp_config(storage).await
            .map_err(|_| "configuration_unavailable")?;
        Self::from_observation(storage, &servers, generation, service)
    }

    pub(crate) fn from_observation(storage: &ResolvedStorage, servers: &[McpServerConfig],
        generation: u64, service: &str) -> Result<Self, &'static str> {
        let matches = servers.iter().filter(|server| server.name == service).collect::<Vec<_>>();
        if matches.len() != 1 { return Err("owned_connector_unavailable"); }
        let config = matches[0];
        let binding = config.connector.as_ref().ok_or("not_connector_oauth")?;
        // An own service can reference a credential owned elsewhere. That is
        // read eligibility, not permission for this realm to change its slot.
        if binding.realm_id != storage.locator.realm.as_str() {
            return Err("credential_owned_by_another_realm");
        }
        if binding.account.is_empty() { return Err("account_unverified"); }
        meerkat::ConnectorMcpInstallation::from_config(config)
            .ok_or("invalid_connector_definition")?
            .map_err(|_| "invalid_connector_definition")?;
        let slot = CredentialAccountRef { realm: storage.locator.realm.clone(),
            account: CredentialAccountId::parse(&binding.slot_id).map_err(|_| "invalid_binding")? };
        Ok(Self { storage:storage.clone(), config:config.clone(), generation, slot })
    }

    pub(crate) fn config(&self) -> &McpServerConfig { &self.config }
    pub(crate) fn slot(&self) -> &CredentialAccountRef { &self.slot }
    pub(crate) fn generation(&self) -> u64 { self.generation }

    pub(crate) fn target(&self, scopes: BTreeSet<String>, client_auth_profile: Option<String>)
        -> ConnectorOAuthTarget {
        let binding = self.config.connector.as_ref().expect("validated connector definition");
        // Scope/profile are a new explicit proposal, never claimed to recover
        // the original login's choices from the service definition.
        ConnectorOAuthTarget { slot:self.slot.clone(), issuer:binding.issuer.clone(),
            client:binding.client.clone(), resource:binding.resource.clone(), scopes,
            strategy_id:binding.strategy_id.clone(), account:AccountSelection::Known(binding.account.clone()),
            client_auth_profile }
    }

    pub(crate) fn check_account(&self, status: &ConnectorAuthStatus) -> Result<(), &'static str> {
        if &status.slot != self.slot() { return Err("invalid_account_observation"); }
        if let Some(account) = &status.verified_account {
            let binding = self.config.connector.as_ref().expect("validated connector definition");
            if account.subject != binding.account || account.issuer != binding.issuer
                || account.strategy_id != binding.strategy_id {
                return Err("different_account_requires_separate_review");
            }
        }
        Ok(())
    }

    pub(crate) async fn check_current(&self) -> Result<(), &'static str> {
        let (servers, generation) = crate::realm::read_own_mcp_config(&self.storage).await
            .map_err(|_| "configuration_unavailable")?;
        if generation != self.generation
            || !servers.iter().filter(|server| server.name == self.config.name)
                .eq(std::iter::once(&self.config)) {
            return Err("reviewed_definition_changed");
        }
        Ok(())
    }

    pub(crate) async fn check_target(&self, target: &ConnectorOAuthTarget) -> Result<(), &'static str> {
        let expected = self.target(target.scopes.clone(), target.client_auth_profile.clone());
        if target.slot != expected.slot || target.issuer != expected.issuer || target.client != expected.client
            || target.resource != expected.resource || target.strategy_id != expected.strategy_id
            || target.account.known() != expected.account.known() || target.account.is_unverified() {
            return Err("invalid_binding");
        }
        self.check_current().await
    }
}
