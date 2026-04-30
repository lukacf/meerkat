//! Runtime OAuth login-flow lifecycle authority.
//!
//! REST/RPC surfaces reach browser-PKCE and device-code admission/consume
//! through [`MeerkatMachine`](crate::meerkat_machine::MeerkatMachine), not by
//! constructing an auth-core registry directly. The auth-core type remains the
//! in-memory state container; this handle is the runtime-owned seam that keeps
//! login-flow lifecycle ownership aligned with the AuthMachine perimeter.

use std::time::Duration;

use meerkat_auth_core::oauth_flow::{
    OAuthDeviceFlowRecord, OAuthDevicePollLease, OAuthFlowAuthority, OAuthFlowError,
    OAuthFlowRecord, OAuthFlowRegistry, OAuthProviderIdentity,
};

#[derive(Debug, Default)]
pub struct RuntimeOAuthFlowHandle {
    registry: OAuthFlowRegistry,
}

impl RuntimeOAuthFlowHandle {
    pub fn new(ttl: Duration) -> Self {
        Self {
            registry: OAuthFlowRegistry::new(ttl),
        }
    }

    pub fn new_with_capacity(ttl: Duration, max_outstanding: usize) -> Self {
        Self {
            registry: OAuthFlowRegistry::new_with_capacity(ttl, max_outstanding),
        }
    }
}

impl OAuthFlowAuthority for RuntimeOAuthFlowHandle {
    fn start(
        &self,
        provider: OAuthProviderIdentity,
        redirect_uri: String,
        pkce_verifier: String,
    ) -> Result<String, OAuthFlowError> {
        self.registry.start(provider, redirect_uri, pkce_verifier)
    }

    fn consume(
        &self,
        state: &str,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError> {
        self.registry.consume(state, provider, redirect_uri)
    }

    fn admit_device_code(
        &self,
        provider: OAuthProviderIdentity,
        device_code: String,
        expires_in: Duration,
    ) -> Result<(), OAuthFlowError> {
        self.registry
            .admit_device_code(provider, device_code, expires_in)
    }

    fn verify_device_code(
        &self,
        device_code: &str,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
        self.registry.verify_device_code(device_code, provider)
    }

    fn begin_device_code_poll(
        &self,
        device_code: &str,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDevicePollLease, OAuthFlowError> {
        self.registry.begin_device_code_poll(device_code, provider)
    }
}
