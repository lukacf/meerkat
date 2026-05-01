//! Runtime OAuth login-flow lifecycle authority.
//!
//! REST/RPC surfaces reach browser-PKCE and device-code admission/consume
//! through [`MeerkatMachine`](crate::meerkat_machine::MeerkatMachine). The
//! auth-core registry stores short-lived PKCE/device payloads; the lifecycle
//! membership and terminal transitions are routed through generated
//! AuthMachine inputs keyed by the target binding.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use meerkat_auth_core::oauth_flow::{
    OAuthDeviceFlowRecord, OAuthDevicePollLease, OAuthDevicePollLifecycle, OAuthFlowAuthority,
    OAuthFlowError, OAuthFlowRecord, OAuthFlowRegistry, OAuthProviderIdentity, OAuthPrunedFlows,
};
use meerkat_core::ConnectionRef;

use crate::auth_machine::dsl as auth_dsl;

use super::RuntimeAuthLeaseHandle;

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn expires_at_millis(duration: Duration) -> Result<u64, OAuthFlowError> {
    let duration_millis =
        u64::try_from(duration.as_millis()).map_err(|_| OAuthFlowError::DeviceExpiryOutOfRange)?;
    current_time_millis()
        .checked_add(duration_millis)
        .ok_or(OAuthFlowError::DeviceExpiryOutOfRange)
}

#[derive(Debug)]
pub struct RuntimeOAuthFlowHandle {
    registry: OAuthFlowRegistry,
    lifecycle: Arc<RuntimeAuthLeaseHandle>,
}

impl RuntimeOAuthFlowHandle {
    pub fn new(ttl: Duration) -> Self {
        Self::new_with_auth_lease(ttl, Arc::new(RuntimeAuthLeaseHandle::new()))
    }

    pub fn new_with_auth_lease(ttl: Duration, lifecycle: Arc<RuntimeAuthLeaseHandle>) -> Self {
        Self {
            registry: OAuthFlowRegistry::new(ttl),
            lifecycle,
        }
    }

    pub fn new_with_capacity(ttl: Duration, max_outstanding: usize) -> Self {
        Self::new_with_capacity_and_auth_lease(
            ttl,
            max_outstanding,
            Arc::new(RuntimeAuthLeaseHandle::new()),
        )
    }

    pub fn new_with_capacity_and_auth_lease(
        ttl: Duration,
        max_outstanding: usize,
        lifecycle: Arc<RuntimeAuthLeaseHandle>,
    ) -> Self {
        Self {
            registry: OAuthFlowRegistry::new_with_capacity(ttl, max_outstanding),
            lifecycle,
        }
    }

    fn apply(
        &self,
        target: &ConnectionRef,
        input: auth_dsl::AuthMachineInput,
        operation: &'static str,
        create_if_missing: bool,
    ) -> Result<(), OAuthFlowError> {
        self.lifecycle
            .apply_oauth_input(target, input, operation, create_if_missing)
            .map_err(|err| OAuthFlowError::LifecycleRejected {
                operation,
                detail: err.to_string(),
            })
    }

    fn admit_browser(
        &self,
        target: &ConnectionRef,
        flow_id: &str,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
        expires_at_millis: u64,
    ) -> Result<(), OAuthFlowError> {
        self.apply(
            target,
            auth_dsl::AuthMachineInput::AdmitOAuthBrowserFlow {
                flow_id: flow_id.to_string(),
                provider: provider.canonical_alias().to_string(),
                redirect_uri: redirect_uri.to_string(),
                expires_at_millis,
                max_outstanding_flows: self.registry.max_outstanding() as u64,
            },
            "admit_oauth_browser_flow",
            true,
        )
    }

    fn verify_browser(
        &self,
        target: &ConnectionRef,
        flow_id: &str,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<(), OAuthFlowError> {
        self.apply(
            target,
            auth_dsl::AuthMachineInput::VerifyOAuthBrowserFlow {
                flow_id: flow_id.to_string(),
                provider: provider.canonical_alias().to_string(),
                redirect_uri: redirect_uri.to_string(),
                now_millis: current_time_millis(),
            },
            "verify_oauth_browser_flow",
            false,
        )
    }

    fn consume_browser(
        &self,
        target: &ConnectionRef,
        flow_id: &str,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<(), OAuthFlowError> {
        self.apply(
            target,
            auth_dsl::AuthMachineInput::ConsumeOAuthBrowserFlow {
                flow_id: flow_id.to_string(),
                provider: provider.canonical_alias().to_string(),
                redirect_uri: redirect_uri.to_string(),
                now_millis: current_time_millis(),
            },
            "consume_oauth_browser_flow",
            false,
        )
    }

    fn expire_browser(&self, target: &ConnectionRef, flow_id: &str) -> Result<(), OAuthFlowError> {
        self.apply(
            target,
            auth_dsl::AuthMachineInput::ExpireOAuthBrowserFlow {
                flow_id: flow_id.to_string(),
            },
            "expire_oauth_browser_flow",
            false,
        )
    }

    fn admit_device(
        &self,
        target: &ConnectionRef,
        flow_id: &str,
        provider: OAuthProviderIdentity,
        expires_at_millis: u64,
    ) -> Result<(), OAuthFlowError> {
        self.apply(
            target,
            auth_dsl::AuthMachineInput::AdmitOAuthDeviceFlow {
                flow_id: flow_id.to_string(),
                provider: provider.canonical_alias().to_string(),
                expires_at_millis,
                max_outstanding_flows: self.registry.max_outstanding() as u64,
            },
            "admit_oauth_device_flow",
            true,
        )
    }

    fn verify_device(
        &self,
        target: &ConnectionRef,
        flow_id: &str,
        provider: OAuthProviderIdentity,
    ) -> Result<(), OAuthFlowError> {
        self.apply(
            target,
            auth_dsl::AuthMachineInput::VerifyOAuthDeviceFlow {
                flow_id: flow_id.to_string(),
                provider: provider.canonical_alias().to_string(),
                now_millis: current_time_millis(),
            },
            "verify_oauth_device_flow",
            false,
        )
    }

    fn begin_device_poll(
        &self,
        target: &ConnectionRef,
        flow_id: &str,
        provider: OAuthProviderIdentity,
    ) -> Result<(), OAuthFlowError> {
        self.apply(
            target,
            auth_dsl::AuthMachineInput::BeginOAuthDevicePoll {
                flow_id: flow_id.to_string(),
                provider: provider.canonical_alias().to_string(),
                now_millis: current_time_millis(),
            },
            "begin_oauth_device_poll",
            false,
        )
    }

    fn expire_pruned_flows(&self) {
        self.expire_collected_flows(OAuthPrunedFlows {
            browser: self.registry.prune_expired_browser_flows(),
            device: self.registry.prune_expired_device_flows(),
        });
    }

    fn expire_collected_flows(&self, pruned: OAuthPrunedFlows) {
        for (flow_id, target) in pruned.browser {
            let _ = self.expire_browser(&target, &flow_id);
        }
        for (device_code, target) in pruned.device {
            let _ = self.lifecycle.expire_device_flow(&target, &device_code);
        }
    }
}

impl Default for RuntimeOAuthFlowHandle {
    fn default() -> Self {
        Self::new(Duration::from_secs(10 * 60))
    }
}

impl OAuthDevicePollLifecycle for RuntimeAuthLeaseHandle {
    fn finish_device_poll(
        &self,
        target: &ConnectionRef,
        device_code: &str,
    ) -> Result<(), OAuthFlowError> {
        self.apply_oauth_input(
            target,
            auth_dsl::AuthMachineInput::FinishOAuthDevicePoll {
                flow_id: device_code.to_string(),
            },
            "finish_oauth_device_poll",
            false,
        )
        .map_err(|err| OAuthFlowError::LifecycleRejected {
            operation: "finish_oauth_device_poll",
            detail: err.to_string(),
        })
    }

    fn consume_device_flow(
        &self,
        target: &ConnectionRef,
        device_code: &str,
        provider: OAuthProviderIdentity,
    ) -> Result<(), OAuthFlowError> {
        self.apply_oauth_input(
            target,
            auth_dsl::AuthMachineInput::ConsumeOAuthDeviceFlow {
                flow_id: device_code.to_string(),
                provider: provider.canonical_alias().to_string(),
                now_millis: current_time_millis(),
            },
            "consume_oauth_device_flow",
            false,
        )
        .map_err(|err| OAuthFlowError::LifecycleRejected {
            operation: "consume_oauth_device_flow",
            detail: err.to_string(),
        })
    }

    fn expire_device_flow(
        &self,
        target: &ConnectionRef,
        device_code: &str,
    ) -> Result<(), OAuthFlowError> {
        self.apply_oauth_input(
            target,
            auth_dsl::AuthMachineInput::ExpireOAuthDeviceFlow {
                flow_id: device_code.to_string(),
            },
            "expire_oauth_device_flow",
            false,
        )
        .map_err(|err| OAuthFlowError::LifecycleRejected {
            operation: "expire_oauth_device_flow",
            detail: err.to_string(),
        })
    }
}

impl OAuthFlowAuthority for RuntimeOAuthFlowHandle {
    fn start(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: String,
        pkce_verifier: String,
    ) -> Result<String, OAuthFlowError> {
        self.expire_pruned_flows();
        let state = OAuthFlowRegistry::new_state()?;
        let expires_at = expires_at_millis(self.registry.ttl())?;
        self.admit_browser(&target, &state, provider, &redirect_uri, expires_at)?;
        let pruned = self.registry.insert_browser_flow_with_pruned(
            state.clone(),
            target,
            provider,
            redirect_uri,
            pkce_verifier,
        );
        self.expire_collected_flows(pruned);
        Ok(state)
    }

    fn verify(
        &self,
        state: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError> {
        self.expire_pruned_flows();
        if let Err(machine_err) = self.verify_browser(target, state, provider, redirect_uri) {
            return match self.registry.verify(state, target, provider, redirect_uri) {
                Err(OAuthFlowError::Missing) => {
                    let _ = self.expire_browser(target, state);
                    Err(OAuthFlowError::Missing)
                }
                _ => Err(machine_err),
            };
        }
        match self.registry.verify(state, target, provider, redirect_uri) {
            Ok(record) => Ok(record),
            Err(OAuthFlowError::Missing) => {
                let _ = self.expire_browser(target, state);
                Err(OAuthFlowError::Missing)
            }
            Err(err) => Err(err),
        }
    }

    fn consume(
        &self,
        state: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError> {
        self.expire_pruned_flows();
        if let Err(machine_err) = self.verify_browser(target, state, provider, redirect_uri) {
            return match self.registry.verify(state, target, provider, redirect_uri) {
                Err(OAuthFlowError::Missing) => {
                    let _ = self.expire_browser(target, state);
                    Err(OAuthFlowError::Missing)
                }
                _ => Err(machine_err),
            };
        }
        let record = match self.registry.verify(state, target, provider, redirect_uri) {
            Ok(record) => record,
            Err(err) => {
                if matches!(err, OAuthFlowError::Missing) {
                    let _ = self.expire_browser(target, state);
                }
                return Err(err);
            }
        };
        self.consume_browser(target, state, provider, redirect_uri)?;
        self.registry
            .consume(state, target, provider, redirect_uri)?;
        Ok(record)
    }

    fn admit_device_code(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        device_code: String,
        expires_in: Duration,
    ) -> Result<(), OAuthFlowError> {
        self.expire_pruned_flows();
        let machine_expires_at = expires_at_millis(expires_in)?;
        if let Err(err) = self.admit_device(&target, &device_code, provider, machine_expires_at) {
            if self
                .lifecycle
                .expire_device_flow(&target, &device_code)
                .is_ok()
                && self
                    .admit_device(&target, &device_code, provider, machine_expires_at)
                    .is_ok()
            {
                // Recovered stale AuthMachine membership left behind after
                // registry-only cleanup; continue with payload insertion.
            } else {
                return Err(err);
            }
        }
        match self
            .registry
            .admit_device_code_with_pruned_without_capacity(
                target.clone(),
                provider,
                device_code.clone(),
                expires_in,
            ) {
            Ok(pruned) => self.expire_collected_flows(pruned),
            Err(err) => {
                let _ = self.lifecycle.expire_device_flow(&target, &device_code);
                return Err(err);
            }
        }
        Ok(())
    }

    fn verify_device_code(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
        self.expire_pruned_flows();
        self.verify_device(target, device_code, provider)?;
        match self
            .registry
            .verify_device_code(device_code, target, provider)
        {
            Ok(record) => Ok(record),
            Err(OAuthFlowError::Missing) => {
                let _ = self.lifecycle.expire_device_flow(target, device_code);
                Err(OAuthFlowError::Missing)
            }
            Err(err) => Err(err),
        }
    }

    fn begin_device_code_poll(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDevicePollLease, OAuthFlowError> {
        self.expire_pruned_flows();
        if let Err(machine_err) = self.begin_device_poll(target, device_code, provider) {
            return match self
                .registry
                .begin_device_code_poll(device_code, target, provider)
            {
                Err(OAuthFlowError::DevicePollInProgress) => {
                    Err(OAuthFlowError::DevicePollInProgress)
                }
                Err(OAuthFlowError::Missing) => {
                    let _ = self.lifecycle.expire_device_flow(target, device_code);
                    Err(OAuthFlowError::Missing)
                }
                Ok(poll) => {
                    drop(poll);
                    Err(machine_err)
                }
                Err(_) => Err(machine_err),
            };
        }
        let poll = match self
            .registry
            .begin_device_code_poll(device_code, target, provider)
        {
            Ok(poll) => poll,
            Err(OAuthFlowError::Missing) => {
                let _ = self.lifecycle.expire_device_flow(target, device_code);
                return Err(OAuthFlowError::Missing);
            }
            Err(err) => {
                let _ = self.lifecycle.finish_device_poll(target, device_code);
                return Err(err);
            }
        };
        let lifecycle: Arc<dyn OAuthDevicePollLifecycle> = self.lifecycle.clone();
        Ok(poll.with_lifecycle(lifecycle))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use meerkat_core::handles::{AuthLeaseHandle, AuthLeasePhase, LeaseKey};

    use super::*;

    fn target() -> ConnectionRef {
        ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("default_openai").expect("valid binding"),
            profile: None,
        }
    }

    fn alternate_target() -> ConnectionRef {
        ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("secondary_openai").expect("valid binding"),
            profile: None,
        }
    }

    fn snapshot_phase(
        lifecycle: &RuntimeAuthLeaseHandle,
        target: &ConnectionRef,
    ) -> Option<AuthLeasePhase> {
        lifecycle
            .snapshot(&LeaseKey::from_connection_ref(target))
            .phase
    }

    #[test]
    fn browser_flow_only_machine_stays_reauth_required_until_credentials_commit() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority =
            RuntimeOAuthFlowHandle::new_with_auth_lease(Duration::from_secs(60), lifecycle.clone());
        let target = target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";

        let state = authority
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "verifier".to_string(),
            )
            .expect("browser flow admitted");
        assert_eq!(
            snapshot_phase(&lifecycle, &target),
            Some(AuthLeasePhase::ReauthRequired)
        );

        authority
            .verify(&state, &target, provider, redirect_uri)
            .expect("browser flow verifies");
        assert_eq!(
            snapshot_phase(&lifecycle, &target),
            Some(AuthLeasePhase::ReauthRequired)
        );

        authority
            .consume(&state, &target, provider, redirect_uri)
            .expect("browser flow consumes");
        assert_eq!(
            snapshot_phase(&lifecycle, &target),
            Some(AuthLeasePhase::ReauthRequired)
        );
    }

    #[test]
    fn global_browser_expiry_preserves_reauth_required_phase() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_auth_lease(
            Duration::from_millis(1),
            lifecycle.clone(),
        );
        let target = target();
        let other_target = alternate_target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";

        let expired_state = authority
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "verifier-old".to_string(),
            )
            .expect("browser flow admitted");
        assert_eq!(
            snapshot_phase(&lifecycle, &target),
            Some(AuthLeasePhase::ReauthRequired)
        );
        std::thread::sleep(Duration::from_millis(10));

        authority
            .start(
                other_target,
                provider,
                redirect_uri.to_string(),
                "verifier-new".to_string(),
            )
            .expect("new browser flow admitted after pruning expired flow");

        assert!(
            !lifecycle.has_oauth_browser_flow_for_test(&target, &expired_state),
            "passive registry expiry must remove stale AuthMachine browser membership"
        );
        assert_eq!(
            snapshot_phase(&lifecycle, &target),
            Some(AuthLeasePhase::ReauthRequired),
            "global OAuth expiry cleanup must not change credential lifecycle truth"
        );
    }

    #[test]
    fn browser_passive_expiry_clears_lifecycle_membership_on_next_admit() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_auth_lease(
            Duration::from_millis(1),
            lifecycle.clone(),
        );
        let target = target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";

        let expired_state = authority
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "verifier-old".to_string(),
            )
            .expect("browser flow admitted");
        assert!(lifecycle.has_oauth_browser_flow_for_test(&target, &expired_state));
        std::thread::sleep(Duration::from_millis(10));

        authority
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "verifier-new".to_string(),
            )
            .expect("new browser flow admitted after pruning expired flow");

        assert!(
            !lifecycle.has_oauth_browser_flow_for_test(&target, &expired_state),
            "passive registry expiry must remove stale AuthMachine browser membership"
        );
    }

    #[test]
    fn device_admit_recovers_registry_pruned_stale_lifecycle_membership() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority =
            RuntimeOAuthFlowHandle::new_with_auth_lease(Duration::from_secs(60), lifecycle.clone());
        let target = target();
        let provider = OAuthProviderIdentity::GoogleCodeAssist;
        let device_code = "provider-device-code";

        authority
            .admit_device_code(
                target.clone(),
                provider,
                device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("device flow admitted");
        assert!(lifecycle.has_oauth_device_flow_for_test(&target, device_code));
        authority
            .registry
            .expire_device_code(device_code, &target, provider)
            .expect("test removes registry record without lifecycle cleanup");
        assert!(lifecycle.has_oauth_device_flow_for_test(&target, device_code));

        authority
            .admit_device_code(
                target.clone(),
                provider,
                device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("stale lifecycle membership is expired before readmit");

        assert!(lifecycle.has_oauth_device_flow_for_test(&target, device_code));
    }

    #[test]
    fn browser_capacity_rejection_comes_from_authmachine_lifecycle() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_capacity_and_auth_lease(
            Duration::from_secs(60),
            1,
            lifecycle,
        );
        let target = target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";

        authority
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "verifier-1".to_string(),
            )
            .expect("first browser flow admitted");

        assert!(matches!(
            authority.start(
                alternate_target(),
                provider,
                "http://127.0.0.1/other-callback".to_string(),
                "verifier-2".to_string(),
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "admit_oauth_browser_flow",
                ..
            })
        ));
    }

    #[test]
    fn browser_provider_mismatch_rejection_comes_from_authmachine_lifecycle() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority =
            RuntimeOAuthFlowHandle::new_with_auth_lease(Duration::from_secs(60), lifecycle);
        let target = target();
        let redirect_uri = "http://127.0.0.1/callback";
        let state = authority
            .start(
                target.clone(),
                OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "verifier".to_string(),
            )
            .expect("browser flow admitted");

        assert!(matches!(
            authority.verify(
                &state,
                &target,
                OAuthProviderIdentity::GoogleCodeAssist,
                redirect_uri,
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "verify_oauth_browser_flow",
                ..
            })
        ));
    }
}
