//! Join one checked software job to actual native account evidence.
//! This prepares a distinct host configuration; it does not persist, attach,
//! grant access, or rewrite the software owner's receipted configuration.

use std::collections::BTreeMap;

use meerkat_core::{McpServerConfig, mcp_config::{McpConnectorBinding, McpConnectorOperation}};

use crate::service_install::InstalledConnectorSelection;

pub(crate) struct InstallationLogin {
    selected: InstalledConnectorSelection,
    derived: McpServerConfig,
    operations: BTreeMap<String, McpConnectorOperation>,
}

impl InstallationLogin {
    pub(crate) fn prepare(
        selected: InstalledConnectorSelection,
        operations: BTreeMap<String, McpConnectorOperation>,
    ) -> Result<Self, &'static str> {
        selected.check_current().map_err(|_| "installation_unconfirmed")?;
        if operations.is_empty() || operations.len() > 128
            || operations.keys().any(|name| !selected.raw_operations.contains(name))
            || operations.values().any(|op| op.required_scopes.len() > 128
                || op.required_scopes.iter().any(|s| s.is_empty() || s.len() > 2048
                    || s.chars().any(char::is_control))
                || op.destination_origin.len() > 2048) {
            return Err("invalid_operation_binding");
        }
        let entry = selected.native_connector.as_ref().ok_or("unsupported_native_entry")?;
        let derived = native_launch_configuration(&selected.installed, &selected.configuration, entry)?;
        Ok(Self { selected, derived, operations })
    }

    pub(crate) fn check_target(&self, target: &meerkat::ConnectorOAuthTarget)
        -> Result<(), &'static str>
    {
        self.selected.check_current().map_err(|_| "installation_unconfirmed")?;
        if self.selected.installed.realm.realm != target.slot.realm {
            return Err("invalid_binding");
        }
        Ok(())
    }

    /// Current software observation is rechecked after login. Authentication
    /// can already have completed when this fails; retain that native result.
    pub(crate) fn join(
        &self,
        target: &meerkat::ConnectorOAuthTarget,
        complete: &meerkat::ConnectorLoginComplete,
    ) -> Result<meerkat::ConnectorMcpInstallation, &'static str> {
        self.join_account(target, &complete.slot, &complete.verified_account)
    }

    /// Reuse a secret-free observation from the native account owner. This does
    /// not complete a login or attest current invocation authority. The native
    /// finalizer must observe the account again after setup closes.
    pub(crate) fn join_authorized(
        &self,
        target: &meerkat::ConnectorOAuthTarget,
        status: &meerkat::ConnectorAuthStatus,
    ) -> Result<meerkat::ConnectorMcpInstallation, &'static str> {
        let account = authorized_account(target, status)?;
        self.join_account(target, &status.slot, account)
    }

    fn join_account(
        &self,
        target: &meerkat::ConnectorOAuthTarget,
        slot: &meerkat_core::connection::CredentialAccountRef,
        account: &meerkat::ConnectorVerifiedAccount,
    ) -> Result<meerkat::ConnectorMcpInstallation, &'static str> {
        self.check_target(target)?;
        check_account(target, slot, account)?;
        let mut config = self.derived.clone();
        config.connector = Some(McpConnectorBinding {
            realm_id: slot.realm.as_str().to_owned(),
            slot_id: slot.account.as_str().to_owned(),
            issuer: target.issuer.clone(),
            client: target.client.clone(),
            resource: target.resource.clone(),
            strategy_id: target.strategy_id.clone(),
            account: account.subject.clone(),
            operations: self.operations.clone(),
        });
        meerkat::ConnectorMcpInstallation::from_config(&config)
            .ok_or("invalid_binding")?
            .map_err(|_| "invalid_operation_binding")
    }

    pub(crate) fn check_current(&self) -> Result<(), &'static str> {
        self.selected.check_current().map_err(|_| "installation_unconfirmed")
    }

    #[cfg(test)]
    pub(crate) fn original(&self) -> &serde_json::Value { &self.selected.configuration }
}

/// Shared mechanical projection of the package owner's verified native entry.
/// Account and access fields are added by their existing owners, never here.
pub(crate) fn native_launch_configuration(
    installed: &crate::service_install::InstalledSelection,
    configuration: &serde_json::Value,
    entry: &crate::service_install::NativeConnectorEntry,
) -> Result<McpServerConfig, &'static str> {
    let root = installed.mcp_path.parent().and_then(std::path::Path::parent)
        .ok_or("unsupported_native_entry")?;
    let command = root.join("runtime/bin/python").to_str()
        .ok_or("unsupported_native_entry")?.to_owned();
    let path = root.join("configuration/selected.json").to_str()
        .ok_or("unsupported_native_entry")?.to_owned();
    let args = vec!["-I".to_owned(), "-m".to_owned(), entry.module.clone(),
        "serve".to_owned(), entry.configuration_argument.clone(), path];
    let observed = &configuration["mcpServers"][&installed.server_name];
    let expected = serde_json::json!({"command":command,"args":args});
    if configuration.as_object().is_none_or(|o| o.len() != 1)
        || configuration["mcpServers"].as_object().is_none_or(|o| o.len() != 1)
        || observed != &expected { return Err("unsupported_native_entry"); }
    let mut native_args = args;
    native_args[3] = entry.mode.clone();
    Ok(McpServerConfig::stdio(&installed.server_name, command, native_args, Default::default()))
}

/// Apply the exact reviewed raw-operation ceiling to a separate native launch
/// descriptor. This does not persist, attach, or grant any access.
pub(crate) fn reviewed_configuration(
    installation: &meerkat::ConnectorMcpInstallation,
) -> Result<McpServerConfig, &'static str> {
    let mut config = installation.config().clone();
    let operations = config.connector.as_ref().ok_or("invalid_binding")?
        .operations.keys().cloned().collect::<Vec<_>>();
    config.access = Some(meerkat_core::mcp_config::McpServerAccess::allowing(operations));
    config.validate_access().map_err(|_| "invalid_operation_binding")?;
    Ok(config)
}

/// Account selection is checked against the native observation, never the slot
/// name. Scopes and access permission remain with the native finalizer/owners.
pub(crate) fn authorized_account<'a>(
    target: &meerkat::ConnectorOAuthTarget,
    status: &'a meerkat::ConnectorAuthStatus,
) -> Result<&'a meerkat::ConnectorVerifiedAccount, &'static str> {
    if status.phase != meerkat::ConnectorAuthPhase::Authorized {
        return Err("account_not_authorized");
    }
    let account = status
        .verified_account
        .as_ref()
        .ok_or("account_unverified")?;
    check_account(target, &status.slot, account)?;
    Ok(account)
}

fn check_account(
    target: &meerkat::ConnectorOAuthTarget,
    slot: &meerkat_core::connection::CredentialAccountRef,
    account: &meerkat::ConnectorVerifiedAccount,
) -> Result<(), &'static str> {
    if slot != &target.slot
        || account.issuer != target.issuer
        || account.strategy_id != target.strategy_id
        || target
            .account
            .known()
            .is_some_and(|selected| selected != account.subject)
        || target.account.is_unverified()
    {
        return Err("invalid_binding");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_auth_core::connector_oauth::AccountSelection;
    use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef, RealmId};

    fn account_observation() -> (meerkat::ConnectorOAuthTarget, meerkat::ConnectorAuthStatus) {
        let slot = CredentialAccountRef {
            realm: RealmId::parse("work").unwrap(),
            account: CredentialAccountId::parse("mail").unwrap(),
        };
        let target = meerkat::ConnectorOAuthTarget {
            slot: slot.clone(),
            issuer: "https://issuer.invalid".into(),
            client: "public-client".into(),
            resource: "https://api.invalid".into(),
            scopes: ["read".into()].into(),
            strategy_id: "oidc-userinfo".into(),
            account: AccountSelection::Discover,
            client_auth_profile: None,
        };
        let status = meerkat::ConnectorAuthStatus {
            slot,
            phase: meerkat::ConnectorAuthPhase::Authorized,
            verified_account: Some(meerkat::ConnectorVerifiedAccount {
                issuer: target.issuer.clone(),
                strategy_id: target.strategy_id.clone(),
                subject: "actual-account".into(),
            }),
            scopes: vec![],
            scope_evidence: None,
            expires_at: None,
            has_refresh_token: false,
        };
        (target, status)
    }

    #[test]
    fn authorized_account_requires_native_verified_identity_and_phase() {
        // Shape-only observations exercise mapping, not actual authentication.
        let (target, mut status) = account_observation();
        assert_eq!(
            authorized_account(&target, &status).unwrap().subject,
            "actual-account"
        );
        for phase in [
            meerkat::ConnectorAuthPhase::ReauthRequired,
            meerkat::ConnectorAuthPhase::AuthorizationRequired,
        ] {
            status.phase = phase;
            assert_eq!(
                authorized_account(&target, &status),
                Err("account_not_authorized")
            );
        }
        status.phase = meerkat::ConnectorAuthPhase::Authorized;
        status.verified_account = None;
        assert_eq!(
            authorized_account(&target, &status),
            Err("account_unverified")
        );
    }

    #[test]
    fn authorized_account_rejects_other_slot_issuer_strategy_and_selected_subject() {
        let (target, status) = account_observation();
        let mut changed = status.clone();
        changed.slot.account = CredentialAccountId::parse("other").unwrap();
        assert_eq!(
            authorized_account(&target, &changed),
            Err("invalid_binding")
        );
        let mut changed = status.clone();
        changed.slot.realm = RealmId::parse("other").unwrap();
        assert_eq!(
            authorized_account(&target, &changed),
            Err("invalid_binding")
        );
        for field in ["issuer", "strategy"] {
            let mut changed = status.clone();
            let account = changed.verified_account.as_mut().unwrap();
            if field == "issuer" {
                account.issuer = "https://other.invalid".into();
            } else {
                account.strategy_id = "other".into();
            }
            assert_eq!(
                authorized_account(&target, &changed),
                Err("invalid_binding")
            );
        }
        let mut target = target;
        target.account = AccountSelection::Known("other-account".into());
        assert_eq!(authorized_account(&target, &status), Err("invalid_binding"));
        target.account = AccountSelection::Known("actual-account".into());
        assert!(authorized_account(&target, &status).is_ok());
        target.account = AccountSelection::Unverified;
        assert_eq!(authorized_account(&target, &status), Err("invalid_binding"));
    }
}
