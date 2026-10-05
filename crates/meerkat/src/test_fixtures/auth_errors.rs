//! One native auth error per public reason, shared by the auth surface tests
//! (`test-mcp-oauth-fixtures`). Test-support only.

use meerkat_contracts::WireAuthErrorReason as R;
use meerkat_core::ConnectionTargetError;
use meerkat_core::connection::{ProviderBindingError, WriteOwnerError};
use meerkat_providers::auth_store::{
    CredentialMutationError, CredentialSlotRefusal, TokenStoreError,
};
use meerkat_providers::connector_login::ConnectorLoginError;
use meerkat_providers::connector_oauth::ConnectorOAuthRefusal;
use meerkat_providers::mcp_oauth::McpOAuthError;
use meerkat_providers::oauth_flow::OAuthFlowError;

use crate::{HostAuthError, HostMcpTargetRefusal};

/// Planted in infrastructure errors: it must never reach public text.
pub const INTERNAL_DETAIL_CANARY: &str = "internal-detail-canary-5f2e";

/// Every public reason, once: an exhaustive match keeps this list complete.
pub fn all_reasons() -> Vec<R> {
    let reasons = vec![
        R::InvalidTarget,
        R::RealmNotFound,
        R::BindingNotFound,
        R::BindingInvalid,
        R::BindingInherited,
        R::FlowUnsupported,
        R::McpServerNotConfigured,
        R::McpServerMismatch,
        R::AccountSelectionRequired,
        R::UnknownStrategy,
        R::AttemptMissing,
        R::AttemptMismatch,
        R::DevicePollInProgress,
        R::DeviceCodeAlreadyAdmitted,
        R::DeviceExpiryInvalid,
        R::AccountMismatch,
        R::MissingScopes,
        R::CredentialMismatch,
        R::VerificationUnavailable,
        R::SlotOccupied,
        R::SlotAccountMismatch,
        R::SlotContextMismatch,
        R::SlotModeMismatch,
        R::UnverifiedConnectorPublication,
        R::ReauthRequired,
        R::AuthorizationRequired,
        R::CallbackUnavailable,
        R::UpstreamFailure,
        R::ConfigurationInvalid,
        R::Infrastructure,
    ];
    for reason in &reasons {
        // Adding a reason without listing it above fails to compile here.
        match reason {
            R::InvalidTarget
            | R::RealmNotFound
            | R::BindingNotFound
            | R::BindingInvalid
            | R::BindingInherited
            | R::FlowUnsupported
            | R::McpServerNotConfigured
            | R::McpServerMismatch
            | R::AccountSelectionRequired
            | R::UnknownStrategy
            | R::AttemptMissing
            | R::AttemptMismatch
            | R::DevicePollInProgress
            | R::DeviceCodeAlreadyAdmitted
            | R::DeviceExpiryInvalid
            | R::AccountMismatch
            | R::MissingScopes
            | R::CredentialMismatch
            | R::VerificationUnavailable
            | R::SlotOccupied
            | R::SlotAccountMismatch
            | R::SlotContextMismatch
            | R::SlotModeMismatch
            | R::UnverifiedConnectorPublication
            | R::ReauthRequired
            | R::AuthorizationRequired
            | R::CallbackUnavailable
            | R::UpstreamFailure
            | R::ConfigurationInvalid
            | R::Infrastructure => {}
        }
    }
    reasons
}

/// A representative native error for every reason (several for some), each
/// with its expected reason.
pub fn reason_examples() -> Vec<(HostAuthError, R)> {
    let binding_error = || ProviderBindingError::UnknownBackend("backend".into());
    vec![
        (
            HostAuthError::ConnectorTarget("bad slot".into()),
            R::InvalidTarget,
        ),
        (
            HostAuthError::Target(ConnectionTargetError::MissingRealm),
            R::InvalidTarget,
        ),
        (
            HostAuthError::Target(ConnectionTargetError::UnknownRealm("r".into())),
            R::RealmNotFound,
        ),
        (
            HostAuthError::Target(ConnectionTargetError::MissingDefaultBinding {
                realm: "r".into(),
            }),
            R::BindingNotFound,
        ),
        (
            HostAuthError::WriteOwner(WriteOwnerError::Unknown {
                binding: "b".into(),
                head: "r".into(),
            }),
            R::BindingNotFound,
        ),
        (
            HostAuthError::Target(ConnectionTargetError::BindingInvalid {
                realm: "r".into(),
                binding: "b".into(),
                source: binding_error(),
            }),
            R::BindingInvalid,
        ),
        (
            HostAuthError::WriteOwner(WriteOwnerError::Inherited {
                binding: "b".into(),
                head: "child".into(),
                owner: "global".into(),
            }),
            R::BindingInherited,
        ),
        (
            HostAuthError::BrowserFlowUnsupported(
                meerkat_core::OAuthProviderIdentity::GitHubCopilot,
            ),
            R::FlowUnsupported,
        ),
        (
            HostAuthError::McpTarget(HostMcpTargetRefusal::UnknownServer {
                server_name: "s".into(),
            }),
            R::McpServerNotConfigured,
        ),
        (
            HostAuthError::McpTarget(HostMcpTargetRefusal::UrlMismatch {
                server_name: "s".into(),
            }),
            R::McpServerMismatch,
        ),
        (
            HostAuthError::McpOAuth(McpOAuthError::AccountSelectionRequired),
            R::AccountSelectionRequired,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::UnknownStrategy),
            R::UnknownStrategy,
        ),
        (
            HostAuthError::OAuthFlow(OAuthFlowError::Missing),
            R::AttemptMissing,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Flow(OAuthFlowError::Missing)),
            R::AttemptMissing,
        ),
        (
            HostAuthError::OAuthFlow(OAuthFlowError::RedirectUriMismatch),
            R::AttemptMismatch,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Verification(
                ConnectorOAuthRefusal::DescriptorMismatch,
            )),
            R::AttemptMismatch,
        ),
        (
            HostAuthError::OAuthFlow(OAuthFlowError::DevicePollInProgress),
            R::DevicePollInProgress,
        ),
        (
            HostAuthError::OAuthFlow(OAuthFlowError::DeviceCodeAlreadyAdmitted),
            R::DeviceCodeAlreadyAdmitted,
        ),
        (
            HostAuthError::OAuthFlow(OAuthFlowError::DeviceExpiryOutOfRange),
            R::DeviceExpiryInvalid,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Verification(
                ConnectorOAuthRefusal::AccountMismatch,
            )),
            R::AccountMismatch,
        ),
        (
            HostAuthError::McpTarget(HostMcpTargetRefusal::AccountMismatch {
                server_name: "s".into(),
            }),
            R::AccountMismatch,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Verification(
                ConnectorOAuthRefusal::MissingScopes,
            )),
            R::MissingScopes,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Verification(
                ConnectorOAuthRefusal::CredentialMismatch,
            )),
            R::CredentialMismatch,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Verification(
                ConnectorOAuthRefusal::VerificationUnavailable,
            )),
            R::VerificationUnavailable,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Slot(CredentialSlotRefusal::Occupied)),
            R::SlotOccupied,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Slot(
                CredentialSlotRefusal::AccountMismatch,
            )),
            R::SlotAccountMismatch,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Slot(
                CredentialSlotRefusal::ContextMismatch,
            )),
            R::SlotContextMismatch,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::Slot(
                CredentialSlotRefusal::ModeMismatch,
            )),
            R::SlotModeMismatch,
        ),
        (
            HostAuthError::CredentialMutation(CredentialMutationError::SlotRefused(
                CredentialSlotRefusal::UnverifiedConnectorPublication,
            )),
            R::UnverifiedConnectorPublication,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::ReauthRequired),
            R::ReauthRequired,
        ),
        (
            HostAuthError::McpOAuth(McpOAuthError::HumanAuthorizationRequired {
                server_name: "s".into(),
            }),
            R::AuthorizationRequired,
        ),
        (
            HostAuthError::McpOAuth(McpOAuthError::Callback {
                server_name: "s".into(),
                reason: "loopback callback listener could not be bound".into(),
            }),
            R::CallbackUnavailable,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::TokenExchangeFailed),
            R::UpstreamFailure,
        ),
        (
            HostAuthError::Target(ConnectionTargetError::RealmConfigInvalid {
                realm: "r".into(),
                source: binding_error(),
            }),
            R::ConfigurationInvalid,
        ),
        (
            HostAuthError::TokenStore(TokenStoreError::Io(INTERNAL_DETAIL_CANARY.into())),
            R::Infrastructure,
        ),
        (
            HostAuthError::OAuthFlow(OAuthFlowError::PersistenceFailed {
                operation: "admit_oauth_browser_flow",
                detail: INTERNAL_DETAIL_CANARY.into(),
            }),
            R::Infrastructure,
        ),
        (
            HostAuthError::Connector(ConnectorLoginError::AuthLifecycle(
                INTERNAL_DETAIL_CANARY.into(),
            )),
            R::Infrastructure,
        ),
    ]
}
