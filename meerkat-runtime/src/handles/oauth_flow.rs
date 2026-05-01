//! Runtime OAuth login-flow lifecycle authority.
//!
//! REST/RPC surfaces reach browser-PKCE and device-code admission/consume
//! through [`MeerkatMachine`](crate::meerkat_machine::MeerkatMachine). The
//! auth-core registry stores short-lived PKCE/device payloads; the lifecycle
//! membership and terminal transitions are routed through generated
//! AuthMachine inputs keyed by the target binding.

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use meerkat_auth_core::oauth_flow::{
    OAuthDeviceFlowRecord, OAuthDevicePollLease, OAuthDevicePollLifecycle, OAuthFlowAuthority,
    OAuthFlowError, OAuthFlowRecord, OAuthFlowRegistry, OAuthFlowRegistrySnapshot,
    OAuthProviderIdentity, OAuthPrunedFlows, PersistedOAuthBrowserFlow, PersistedOAuthDeviceFlow,
};
use meerkat_core::ConnectionRef;

use crate::auth_machine::dsl as auth_dsl;
use crate::store::RuntimeStore;

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
    registry: Arc<OAuthFlowRegistry>,
    lifecycle: Arc<RuntimeAuthLeaseHandle>,
    store: Option<Weak<dyn RuntimeStore>>,
}

impl RuntimeOAuthFlowHandle {
    pub fn new(ttl: Duration) -> Self {
        Self::new_with_auth_lease(ttl, Arc::new(RuntimeAuthLeaseHandle::new()))
    }

    pub fn new_with_auth_lease(ttl: Duration, lifecycle: Arc<RuntimeAuthLeaseHandle>) -> Self {
        Self {
            registry: Arc::new(OAuthFlowRegistry::new(ttl)),
            lifecycle,
            store: None,
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
            registry: Arc::new(OAuthFlowRegistry::new_with_capacity(ttl, max_outstanding)),
            lifecycle,
            store: None,
        }
    }

    pub fn new_with_persistent_store_and_auth_lease(
        ttl: Duration,
        lifecycle: Arc<RuntimeAuthLeaseHandle>,
        store: &Arc<dyn RuntimeStore>,
    ) -> Self {
        Self::new_with_capacity_auth_lease_and_store(
            ttl,
            1024,
            lifecycle,
            Some(Arc::downgrade(store)),
        )
    }

    fn new_with_capacity_auth_lease_and_store(
        ttl: Duration,
        max_outstanding: usize,
        lifecycle: Arc<RuntimeAuthLeaseHandle>,
        store: Option<Weak<dyn RuntimeStore>>,
    ) -> Self {
        let handle = Self {
            registry: Arc::new(OAuthFlowRegistry::new_with_capacity(ttl, max_outstanding)),
            lifecycle,
            store,
        };
        handle.rehydrate_persisted_payloads();
        handle
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

    fn retain_registry_payloads_with_lifecycle(&self) {
        self.registry.retain_flows_with_lifecycle(
            |target, flow_id| self.lifecycle.has_oauth_browser_flow(target, flow_id),
            |target, flow_id| self.lifecycle.has_oauth_device_flow(target, flow_id),
        );
    }

    fn expire_collected_flows(&self, pruned: OAuthPrunedFlows) {
        for (flow_id, target) in pruned.browser {
            let _ = self.expire_browser(&target, &flow_id);
        }
        for (device_code, target) in pruned.device {
            let _ = self.lifecycle.expire_device_flow(&target, &device_code);
        }
    }

    fn store(&self) -> Option<Arc<dyn RuntimeStore>> {
        self.store.as_ref().and_then(Weak::upgrade)
    }

    fn persist_registry_payloads(&self, operation: &'static str) -> Result<(), OAuthFlowError> {
        persist_registry_payloads(&self.registry, self.store.as_ref(), operation)
    }

    fn rehydrate_persisted_payloads(&self) {
        let Some(store) = self.store() else {
            return;
        };
        let Ok(Some(bytes)) = store.load_auth_oauth_flow_snapshot() else {
            return;
        };
        let Ok(snapshot) = serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&bytes) else {
            return;
        };
        let now_millis = current_time_millis();
        let now_instant = Instant::now();

        for persisted in snapshot.browser {
            self.restore_browser_payload(persisted, now_millis, now_instant);
        }
        for persisted in snapshot.device {
            self.restore_device_payload(persisted, now_millis, now_instant);
        }
        let _ = self.persist_registry_payloads("rehydrate_oauth_flows");
    }

    fn restore_browser_payload(
        &self,
        persisted: PersistedOAuthBrowserFlow,
        now_millis: u64,
        now_instant: Instant,
    ) {
        if persisted.expires_at_millis <= now_millis {
            return;
        }
        let Some(provider) = OAuthProviderIdentity::from_alias(&persisted.provider) else {
            return;
        };
        let remaining = Duration::from_millis(persisted.expires_at_millis - now_millis);
        let elapsed = self.registry.ttl().saturating_sub(remaining);
        let created_at = now_instant.checked_sub(elapsed).unwrap_or(now_instant);
        if self
            .admit_browser(
                &persisted.target,
                &persisted.state,
                provider,
                &persisted.redirect_uri,
                persisted.expires_at_millis,
            )
            .is_err()
        {
            return;
        }
        if self
            .registry
            .insert_restored_browser_flow(
                persisted.state.clone(),
                persisted.target.clone(),
                provider,
                persisted.redirect_uri.clone(),
                persisted.pkce_verifier.clone(),
                created_at,
            )
            .is_err()
        {
            let _ = self.expire_browser(&persisted.target, &persisted.state);
        }
    }

    fn restore_device_payload(
        &self,
        persisted: PersistedOAuthDeviceFlow,
        now_millis: u64,
        now_instant: Instant,
    ) {
        if persisted.expires_at_millis <= now_millis {
            return;
        }
        let Some(provider) = OAuthProviderIdentity::from_alias(&persisted.provider) else {
            return;
        };
        let remaining = Duration::from_millis(persisted.expires_at_millis - now_millis);
        let expires_at = now_instant.checked_add(remaining).unwrap_or(now_instant);
        let elapsed = Duration::from_millis(now_millis.saturating_sub(persisted.created_at_millis));
        let created_at = now_instant.checked_sub(elapsed).unwrap_or(now_instant);
        if self
            .admit_device(
                &persisted.target,
                &persisted.device_code,
                provider,
                persisted.expires_at_millis,
            )
            .is_err()
        {
            return;
        }
        if self
            .registry
            .insert_restored_device_flow(
                persisted.target.clone(),
                provider,
                persisted.device_code.clone(),
                created_at,
                expires_at,
            )
            .is_err()
        {
            let _ = self
                .lifecycle
                .expire_device_flow(&persisted.target, &persisted.device_code);
        }
    }
}

fn persist_registry_payloads(
    registry: &OAuthFlowRegistry,
    store: Option<&Weak<dyn RuntimeStore>>,
    operation: &'static str,
) -> Result<(), OAuthFlowError> {
    let Some(store) = store.and_then(Weak::upgrade) else {
        return Ok(());
    };
    let snapshot = registry.snapshot_for_persistence(current_time_millis());
    let bytes = serde_json::to_vec(&snapshot).map_err(|err| OAuthFlowError::PersistenceFailed {
        operation,
        detail: err.to_string(),
    })?;
    store
        .persist_auth_oauth_flow_snapshot(&bytes)
        .map_err(|err| OAuthFlowError::PersistenceFailed {
            operation,
            detail: err.to_string(),
        })
}

struct RuntimeOAuthDevicePollLifecycle {
    lifecycle: Arc<RuntimeAuthLeaseHandle>,
    registry: Arc<OAuthFlowRegistry>,
    store: Option<Weak<dyn RuntimeStore>>,
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

impl OAuthDevicePollLifecycle for RuntimeOAuthDevicePollLifecycle {
    fn finish_device_poll(
        &self,
        target: &ConnectionRef,
        device_code: &str,
    ) -> Result<(), OAuthFlowError> {
        self.lifecycle.finish_device_poll(target, device_code)
    }

    fn consume_device_flow(
        &self,
        target: &ConnectionRef,
        device_code: &str,
        provider: OAuthProviderIdentity,
    ) -> Result<(), OAuthFlowError> {
        self.lifecycle
            .consume_device_flow(target, device_code, provider)
    }

    fn expire_device_flow(
        &self,
        target: &ConnectionRef,
        device_code: &str,
    ) -> Result<(), OAuthFlowError> {
        self.lifecycle.expire_device_flow(target, device_code)
    }

    fn device_flow_payloads_changed(&self) -> Result<(), OAuthFlowError> {
        persist_registry_payloads(
            &self.registry,
            self.store.as_ref(),
            "persist_oauth_device_flow_payloads",
        )
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
        let mut inserted = self.registry.insert_browser_flow_with_pruned(
            state.clone(),
            target.clone(),
            provider,
            redirect_uri.clone(),
            pkce_verifier.clone(),
        );
        if matches!(inserted, Err(OAuthFlowError::CapacityExceeded { .. })) {
            self.retain_registry_payloads_with_lifecycle();
            inserted = self.registry.insert_browser_flow_with_pruned(
                state.clone(),
                target.clone(),
                provider,
                redirect_uri,
                pkce_verifier,
            );
        }
        let pruned = match inserted {
            Ok(pruned) => pruned,
            Err(err) => {
                let _ = self.expire_browser(&target, &state);
                return Err(err);
            }
        };
        self.expire_collected_flows(pruned);
        self.persist_registry_payloads("admit_oauth_browser_flow")?;
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
        self.persist_registry_payloads("consume_oauth_browser_flow")?;
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
            match self
                .registry
                .verify_device_code(&device_code, &target, provider)
            {
                Err(OAuthFlowError::Missing) => {
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
                Ok(_) => return Err(OAuthFlowError::DeviceCodeAlreadyAdmitted),
                Err(_) => return Err(err),
            }
        }
        let mut inserted = self.registry.admit_device_code_with_pruned(
            target.clone(),
            provider,
            device_code.clone(),
            expires_in,
        );
        if matches!(inserted, Err(OAuthFlowError::CapacityExceeded { .. })) {
            self.retain_registry_payloads_with_lifecycle();
            inserted = self.registry.admit_device_code_with_pruned(
                target.clone(),
                provider,
                device_code.clone(),
                expires_in,
            );
        }
        match inserted {
            Ok(pruned) => self.expire_collected_flows(pruned),
            Err(err) => {
                let _ = self.lifecycle.expire_device_flow(&target, &device_code);
                return Err(err);
            }
        }
        self.persist_registry_payloads("admit_oauth_device_flow")?;
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
        let lifecycle: Arc<dyn OAuthDevicePollLifecycle> =
            Arc::new(RuntimeOAuthDevicePollLifecycle {
                lifecycle: Arc::clone(&self.lifecycle),
                registry: Arc::clone(&self.registry),
                store: self.store.clone(),
            });
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
    fn oauth_flow_membership_does_not_advance_credential_generation() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority =
            RuntimeOAuthFlowHandle::new_with_auth_lease(Duration::from_secs(60), lifecycle.clone());
        let target = target();
        let lease_key = LeaseKey::from_connection_ref(&target);
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";
        let transition = lifecycle
            .acquire_lease(&lease_key, 4_200)
            .expect("credential lifecycle acquired");

        let state = authority
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "verifier".to_string(),
            )
            .expect("browser flow admitted");
        authority
            .verify(&state, &target, provider, redirect_uri)
            .expect("browser flow verifies");
        authority
            .consume(&state, &target, provider, redirect_uri)
            .expect("browser flow consumes");

        let snapshot = lifecycle.snapshot(&lease_key);
        assert_eq!(snapshot.generation, transition.generation);
        assert_eq!(
            snapshot.credential_published_at_millis,
            transition.credential_published_at_millis
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
    fn duplicate_device_admit_preserves_active_lifecycle_membership() {
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
        let duplicate = authority.admit_device_code(
            target.clone(),
            provider,
            device_code.to_string(),
            Duration::from_secs(60),
        );

        assert_eq!(duplicate, Err(OAuthFlowError::DeviceCodeAlreadyAdmitted));
        assert!(lifecycle.has_oauth_device_flow_for_test(&target, device_code));
        authority
            .begin_device_code_poll(device_code, &target, provider)
            .expect("duplicate admit must not orphan active lifecycle membership");
    }

    #[test]
    fn registry_capacity_still_bounds_payloads_after_lifecycle_release() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_capacity_and_auth_lease(
            Duration::from_secs(60),
            1,
            lifecycle.clone(),
        );
        let target = target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;

        authority
            .start(
                target.clone(),
                provider,
                "http://127.0.0.1/callback".to_string(),
                "verifier-1".to_string(),
            )
            .expect("first browser flow admitted");
        lifecycle
            .release_lease(&LeaseKey::from_connection_ref(&target))
            .expect("credential lifecycle release succeeds");

        authority
            .start(
                alternate_target(),
                provider,
                "http://127.0.0.1/other-callback".to_string(),
                "verifier-2".to_string(),
            )
            .expect("AuthMachine release must clear stale registry payload capacity");
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
