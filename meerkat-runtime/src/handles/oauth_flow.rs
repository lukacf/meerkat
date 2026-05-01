//! Runtime OAuth login-flow lifecycle authority.
//!
//! REST/RPC surfaces reach browser-PKCE and device-code admission/consume
//! through [`MeerkatMachine`](crate::meerkat_machine::MeerkatMachine). The
//! auth-core registry stores short-lived PKCE/device payloads; the lifecycle
//! membership and terminal transitions are routed through generated
//! AuthMachine inputs keyed by the target binding.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use meerkat_auth_core::oauth_flow::{
    OAuthDeviceFlowRecord, OAuthDevicePollLease, OAuthDevicePollLifecycle, OAuthFlowAuthority,
    OAuthFlowError, OAuthFlowRecord, OAuthFlowRegistry, OAuthFlowRegistrySnapshot,
    OAuthProviderIdentity, OAuthPrunedFlows, PersistedOAuthBrowserFlow, PersistedOAuthDeviceFlow,
};
use meerkat_core::ConnectionRef;
use meerkat_core::handles::DslTransitionError;
#[cfg(test)]
use meerkat_core::handles::LeaseKey;

use crate::auth_machine::dsl as auth_dsl;
use crate::store::RuntimeStore;

use super::RuntimeAuthLeaseHandle;
use super::auth_lease::{AuthLeaseReleaseObserver, ReleasedOAuthFlows};

type StoreSlot = Arc<Mutex<Option<Weak<dyn RuntimeStore>>>>;
type PayloadLock = Arc<Mutex<()>>;

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
    store: StoreSlot,
    payload_lock: PayloadLock,
    _release_observer: Option<Arc<OAuthPayloadReleaseObserver>>,
}

#[derive(Debug)]
struct OAuthPayloadReleaseObserver {
    registry: Arc<OAuthFlowRegistry>,
    store: StoreSlot,
    payload_lock: PayloadLock,
}

impl AuthLeaseReleaseObserver for OAuthPayloadReleaseObserver {
    fn auth_lease_released(&self, released: &ReleasedOAuthFlows) -> Result<(), DslTransitionError> {
        let _payload_guard = self
            .payload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let target = ConnectionRef {
            realm: released.lease_key.realm.clone(),
            binding: released.lease_key.binding.clone(),
            profile: released.lease_key.profile.clone(),
        };
        let browser_flow_ids = released
            .browser_flow_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let device_flow_ids = released
            .device_flow_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let now_millis = current_time_millis();
        let mut snapshot = self.registry.snapshot_for_persistence(now_millis);
        snapshot.browser.retain(|flow| {
            !(flow.target == target && browser_flow_ids.contains(flow.state.as_str()))
        });
        snapshot.device.retain(|flow| {
            !(flow.target == target && device_flow_ids.contains(flow.device_code.as_str()))
        });
        persist_registry_snapshot(&snapshot, &self.store, "release_oauth_flow_payloads").map_err(
            |err| {
                DslTransitionError::new(
                    "AuthLeaseReleaseObserver::release_oauth_flow_payloads",
                    err.to_string(),
                )
            },
        )?;
        self.registry.retain_flows_with_lifecycle(
            |record_target, flow_id| {
                !(record_target == &target && browser_flow_ids.contains(flow_id))
            },
            |record_target, device_code| {
                !(record_target == &target && device_flow_ids.contains(device_code))
            },
        );
        Ok(())
    }
}

impl RuntimeOAuthFlowHandle {
    pub fn new(ttl: Duration) -> Self {
        Self::new_with_auth_lease(ttl, Arc::new(RuntimeAuthLeaseHandle::new()))
    }

    pub fn new_with_auth_lease(ttl: Duration, lifecycle: Arc<RuntimeAuthLeaseHandle>) -> Self {
        Self::new_with_capacity_auth_lease_and_store(ttl, 1024, lifecycle, None)
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
        Self::new_with_capacity_auth_lease_and_store(ttl, max_outstanding, lifecycle, None)
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
        let registry = Arc::new(OAuthFlowRegistry::new_with_capacity(ttl, max_outstanding));
        let store = Arc::new(Mutex::new(store));
        let payload_lock = Arc::new(Mutex::new(()));
        let release_observer = Arc::new(OAuthPayloadReleaseObserver {
            registry: Arc::clone(&registry),
            store: Arc::clone(&store),
            payload_lock: Arc::clone(&payload_lock),
        });
        let release_observer_dyn: Arc<dyn AuthLeaseReleaseObserver> = release_observer.clone();
        lifecycle.add_release_observer(Arc::downgrade(&release_observer_dyn));
        let handle = Self {
            registry,
            lifecycle,
            store,
            payload_lock,
            _release_observer: Some(release_observer),
        };
        handle.rehydrate_persisted_payloads();
        handle
    }

    pub(crate) fn bind_persistent_store(&self, store: &Arc<dyn RuntimeStore>) {
        *self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(store));
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
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }

    fn persist_registry_payloads(&self, operation: &'static str) -> Result<(), OAuthFlowError> {
        persist_registry_payloads(&self.registry, &self.store, operation)
    }

    fn browser_record_expires_at_millis(
        &self,
        record: &OAuthFlowRecord,
    ) -> Result<u64, OAuthFlowError> {
        let remaining = self
            .registry
            .ttl()
            .checked_sub(record.created_at.elapsed())
            .ok_or(OAuthFlowError::Missing)?;
        expires_at_millis(remaining)
    }

    fn restore_browser_flow(
        &self,
        state: &str,
        record: &OAuthFlowRecord,
    ) -> Result<(), OAuthFlowError> {
        let expires_at_millis = self.browser_record_expires_at_millis(record)?;
        self.admit_browser(
            &record.target,
            state,
            record.provider,
            &record.redirect_uri,
            expires_at_millis,
        )?;
        self.registry.insert_restored_browser_flow(
            state.to_string(),
            record.target.clone(),
            record.provider,
            record.redirect_uri.clone(),
            record.pkce_verifier.clone(),
            record.created_at,
        )
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
    store: &StoreSlot,
    operation: &'static str,
) -> Result<(), OAuthFlowError> {
    let snapshot = registry.snapshot_for_persistence(current_time_millis());
    persist_registry_snapshot(&snapshot, store, operation)
}

fn persist_registry_snapshot(
    snapshot: &OAuthFlowRegistrySnapshot,
    store: &StoreSlot,
    operation: &'static str,
) -> Result<(), OAuthFlowError> {
    let store = store
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(store) = store else {
        return Ok(());
    };
    let Some(store) = store.upgrade() else {
        return Err(OAuthFlowError::PersistenceFailed {
            operation,
            detail: "runtime store is no longer available".to_string(),
        });
    };
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
    store: StoreSlot,
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

    fn restore_device_flow(&self, record: &OAuthDeviceFlowRecord) -> Result<(), OAuthFlowError> {
        let remaining = record
            .expires_at
            .checked_duration_since(Instant::now())
            .ok_or(OAuthFlowError::Missing)?;
        let expires_at_millis = expires_at_millis(remaining)?;
        self.apply_oauth_input(
            &record.target,
            auth_dsl::AuthMachineInput::AdmitOAuthDeviceFlow {
                flow_id: record.device_code.clone(),
                provider: record.provider.canonical_alias().to_string(),
                expires_at_millis,
                max_outstanding_flows: u64::MAX,
            },
            "restore_oauth_device_flow",
            true,
        )
        .map_err(|err| OAuthFlowError::LifecycleRejected {
            operation: "restore_oauth_device_flow",
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

    fn restore_device_flow(&self, record: &OAuthDeviceFlowRecord) -> Result<(), OAuthFlowError> {
        let remaining = record
            .expires_at
            .checked_duration_since(Instant::now())
            .ok_or(OAuthFlowError::Missing)?;
        let expires_at_millis = expires_at_millis(remaining)?;
        self.lifecycle
            .apply_oauth_input(
                &record.target,
                auth_dsl::AuthMachineInput::AdmitOAuthDeviceFlow {
                    flow_id: record.device_code.clone(),
                    provider: record.provider.canonical_alias().to_string(),
                    expires_at_millis,
                    max_outstanding_flows: self.registry.max_outstanding() as u64,
                },
                "restore_oauth_device_flow",
                true,
            )
            .map_err(|err| OAuthFlowError::LifecycleRejected {
                operation: "restore_oauth_device_flow",
                detail: err.to_string(),
            })
    }

    fn device_flow_payloads_changed(&self) -> Result<(), OAuthFlowError> {
        persist_registry_payloads(
            &self.registry,
            &self.store,
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
        let _payload_guard = self
            .payload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _payload_guard = self
            .payload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        if let Err(err) = self.registry.consume(state, target, provider, redirect_uri) {
            let _ = self.restore_browser_flow(state, &record);
            return Err(err);
        }
        if let Err(err) = self.persist_registry_payloads("consume_oauth_browser_flow") {
            let _ = self.restore_browser_flow(state, &record);
            return Err(err);
        }
        Ok(record)
    }

    fn admit_device_code(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        device_code: String,
        expires_in: Duration,
    ) -> Result<(), OAuthFlowError> {
        let _payload_guard = self
            .payload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _payload_guard = self
            .payload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                store: Arc::clone(&self.store),
            });
        Ok(poll
            .with_lifecycle(lifecycle)
            .with_operation_lock(Arc::clone(&self.payload_lock)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Condvar, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    };

    use meerkat_core::handles::{AuthLeaseHandle, AuthLeasePhase, LeaseKey};
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
    use meerkat_core::types::SessionId;

    use super::*;
    use crate::identifiers::LogicalRuntimeId;
    use crate::input_state::StoredInputState;
    use crate::runtime_state::RuntimeState;
    use crate::store::{RuntimeStore, RuntimeStoreError, SessionDelta};

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

    #[derive(Debug, Default)]
    struct BlockingOAuthPersistState {
        armed: bool,
        blocked: bool,
        released: bool,
    }

    #[derive(Debug, Default)]
    struct FailingOAuthSnapshotStore {
        snapshot: StdMutex<Option<Vec<u8>>>,
        fail_oauth_persist: AtomicBool,
        blocking_oauth_persist: StdMutex<BlockingOAuthPersistState>,
        blocking_oauth_persist_cv: Condvar,
    }

    impl FailingOAuthSnapshotStore {
        fn block_next_oauth_persist(&self) {
            let mut state = self
                .blocking_oauth_persist
                .lock()
                .expect("blocking persist state lock");
            state.armed = true;
            state.blocked = false;
            state.released = false;
        }

        fn wait_for_blocked_oauth_persist(&self) {
            let mut state = self
                .blocking_oauth_persist
                .lock()
                .expect("blocking persist state lock");
            while !state.blocked {
                let (next, timeout) = self
                    .blocking_oauth_persist_cv
                    .wait_timeout(state, Duration::from_secs(1))
                    .expect("blocking persist state wait");
                assert!(
                    !timeout.timed_out(),
                    "expected OAuth snapshot persist to block"
                );
                state = next;
            }
        }

        fn release_blocked_oauth_persist(&self) {
            let mut state = self
                .blocking_oauth_persist
                .lock()
                .expect("blocking persist state lock");
            state.released = true;
            self.blocking_oauth_persist_cv.notify_all();
        }

        fn wait_if_oauth_persist_blocked(&self) {
            let mut state = self
                .blocking_oauth_persist
                .lock()
                .expect("blocking persist state lock");
            if !state.armed {
                return;
            }
            state.armed = false;
            state.blocked = true;
            self.blocking_oauth_persist_cv.notify_all();
            while !state.released {
                state = self
                    .blocking_oauth_persist_cv
                    .wait(state)
                    .expect("blocking persist state wait");
            }
        }

        fn fail_oauth_persist(&self) {
            self.fail_oauth_persist.store(true, Ordering::SeqCst);
        }

        fn allow_oauth_persist(&self) {
            self.fail_oauth_persist.store(false, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl RuntimeStore for FailingOAuthSnapshotStore {
        fn persist_auth_oauth_flow_snapshot(
            &self,
            snapshot_json: &[u8],
        ) -> Result<(), RuntimeStoreError> {
            self.wait_if_oauth_persist_blocked();
            if self.fail_oauth_persist.load(Ordering::SeqCst) {
                return Err(RuntimeStoreError::WriteFailed(
                    "injected oauth snapshot failure".to_string(),
                ));
            }
            *self
                .snapshot
                .lock()
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))? =
                Some(snapshot_json.to_vec());
            Ok(())
        }

        fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            self.snapshot
                .lock()
                .map(|snapshot| snapshot.clone())
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
        }

        async fn commit_session_boundary(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _session_delta: SessionDelta,
            _run_id: RunId,
            _boundary: RunApplyBoundary,
            _contributing_input_ids: Vec<InputId>,
            _input_updates: Vec<StoredInputState>,
        ) -> Result<RunBoundaryReceipt, RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "commit_session_boundary".to_string(),
            ))
        }

        async fn commit_session_snapshot(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _session_delta: SessionDelta,
        ) -> Result<(), RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "commit_session_snapshot".to_string(),
            ))
        }

        async fn atomic_apply(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _session_delta: Option<SessionDelta>,
            _receipt: RunBoundaryReceipt,
            _input_updates: Vec<StoredInputState>,
            _session_store_key: Option<SessionId>,
        ) -> Result<(), RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported("atomic_apply".to_string()))
        }

        async fn load_input_states(
            &self,
            _runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "load_input_states".to_string(),
            ))
        }

        async fn load_boundary_receipt(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _run_id: &RunId,
            _sequence: u64,
        ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "load_boundary_receipt".to_string(),
            ))
        }

        async fn load_session_snapshot(
            &self,
            _runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "load_session_snapshot".to_string(),
            ))
        }

        async fn persist_input_state(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _state: &StoredInputState,
        ) -> Result<(), RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "persist_input_state".to_string(),
            ))
        }

        async fn load_input_state(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _input_id: &InputId,
        ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "load_input_state".to_string(),
            ))
        }

        async fn persist_runtime_state(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _state: RuntimeState,
        ) -> Result<(), RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "persist_runtime_state".to_string(),
            ))
        }

        async fn load_runtime_state(
            &self,
            _runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<RuntimeState>, RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "load_runtime_state".to_string(),
            ))
        }

        async fn atomic_lifecycle_commit(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _runtime_state: RuntimeState,
            _input_states: &[StoredInputState],
        ) -> Result<(), RuntimeStoreError> {
            Err(RuntimeStoreError::Unsupported(
                "atomic_lifecycle_commit".to_string(),
            ))
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
    fn concurrent_browser_admits_preserve_newer_durable_snapshot() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let authority = Arc::new(
            RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
                Duration::from_secs(60),
                lifecycle,
                &store_dyn,
            ),
        );
        let first_target = target();
        let second_target = alternate_target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let first_redirect_uri = "http://127.0.0.1/callback";
        let second_redirect_uri = "http://127.0.0.1/other-callback";

        store.block_next_oauth_persist();
        let first_admit = std::thread::spawn({
            let authority = Arc::clone(&authority);
            let target = first_target.clone();
            move || {
                authority.start(
                    target,
                    provider,
                    first_redirect_uri.to_string(),
                    "verifier-1".to_string(),
                )
            }
        });
        store.wait_for_blocked_oauth_persist();

        let (second_done_tx, second_done_rx) = std::sync::mpsc::channel();
        std::thread::spawn({
            let authority = Arc::clone(&authority);
            let target = second_target.clone();
            move || {
                let result = authority.start(
                    target,
                    provider,
                    second_redirect_uri.to_string(),
                    "verifier-2".to_string(),
                );
                let _ = second_done_tx.send(result);
            }
        });
        let second_before_release = second_done_rx.recv_timeout(Duration::from_millis(100)).ok();
        store.release_blocked_oauth_persist();
        first_admit
            .join()
            .expect("first admit thread should not panic")
            .expect("first browser flow admitted");
        let second_state = second_before_release
            .unwrap_or_else(|| {
                second_done_rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("second admit should finish after first durable write is released")
            })
            .expect("second browser flow admitted");

        let snapshot_json = store
            .load_auth_oauth_flow_snapshot()
            .expect("durable OAuth snapshot loads")
            .expect("durable OAuth snapshot exists");
        let snapshot = serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&snapshot_json)
            .expect("durable OAuth snapshot decodes");
        assert!(
            snapshot
                .browser
                .iter()
                .any(|flow| flow.state == second_state),
            "durable snapshot must retain flow admitted by a concurrent newer write"
        );

        let restarted_lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let restarted = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            restarted_lifecycle,
            &store_dyn,
        );
        let record = restarted
            .consume(&second_state, &second_target, provider, second_redirect_uri)
            .expect("newer durable flow should survive restart");
        assert_eq!(record.pkce_verifier, "verifier-2");
    }

    #[test]
    fn browser_consume_persistence_failure_keeps_flow_retryable() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            lifecycle.clone(),
            &store_dyn,
        );
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

        store.fail_oauth_persist();
        assert!(matches!(
            authority.consume(&state, &target, provider, redirect_uri),
            Err(OAuthFlowError::PersistenceFailed { .. })
        ));
        assert!(lifecycle.has_oauth_browser_flow_for_test(&target, &state));

        store.allow_oauth_persist();
        authority
            .consume(&state, &target, provider, redirect_uri)
            .expect("failed durable consume remains retryable");
    }

    #[test]
    fn device_consume_persistence_failure_keeps_flow_retryable() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            lifecycle.clone(),
            &store_dyn,
        );
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
        let poll = authority
            .begin_device_code_poll(device_code, &target, provider)
            .expect("device poll begins");

        store.fail_oauth_persist();
        assert!(matches!(
            poll.consume(),
            Err(OAuthFlowError::PersistenceFailed { .. })
        ));
        assert!(lifecycle.has_oauth_device_flow_for_test(&target, device_code));

        store.allow_oauth_persist();
        let retry = authority
            .begin_device_code_poll(device_code, &target, provider)
            .expect("failed durable consume keeps device flow retryable");
        retry
            .consume()
            .expect("retry consumes after durable persistence recovers");
    }

    #[test]
    fn release_persistence_failure_keeps_released_flows_retryable() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            lifecycle.clone(),
            &store_dyn,
        );
        let target = target();
        let lease_key = LeaseKey::from_connection_ref(&target);
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

        store.fail_oauth_persist();
        assert!(
            lifecycle.release_lease(&lease_key).is_err(),
            "release should fail closed when durable OAuth cleanup cannot persist"
        );
        assert!(lifecycle.has_oauth_browser_flow_for_test(&target, &state));

        store.allow_oauth_persist();
        authority
            .consume(&state, &target, provider, redirect_uri)
            .expect("failed durable release leaves browser flow retryable");
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
    fn release_observer_does_not_prune_flow_admitted_after_release_acceptance() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
            Duration::from_secs(60),
            lifecycle.clone(),
        ));
        let target = target();
        let lease_key = LeaseKey::from_connection_ref(&target);
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";
        let old_state = authority
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "old-verifier".to_string(),
            )
            .expect("old browser flow admitted");
        let new_state = Arc::new(std::sync::Mutex::new(None));
        let new_state_for_hook = Arc::clone(&new_state);
        let authority_for_hook = Arc::clone(&authority);
        let target_for_hook = target.clone();
        let lease_key_for_hook = lease_key.clone();
        crate::handles::auth_lease::set_release_after_accept_hook_for_test(Some(Arc::new(
            move |released_key| {
                if released_key != &lease_key_for_hook {
                    return;
                }
                let admitted = authority_for_hook
                    .start(
                        target_for_hook.clone(),
                        provider,
                        redirect_uri.to_string(),
                        "new-verifier".to_string(),
                    )
                    .expect("new browser flow admitted after release acceptance");
                *new_state_for_hook
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(admitted);
            },
        )));

        lifecycle
            .release_lease(&lease_key)
            .expect("credential lifecycle release succeeds");
        crate::handles::auth_lease::set_release_after_accept_hook_for_test(None);

        let new_state = new_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("hook admitted replacement flow");
        let flow = authority
            .consume(&new_state, &target, provider, redirect_uri)
            .expect("release observer must not prune newly admitted flow");
        assert_eq!(flow.pkce_verifier, "new-verifier");
        assert!(matches!(
            authority.consume(&old_state, &target, provider, redirect_uri),
            Err(OAuthFlowError::Missing)
        ));
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
