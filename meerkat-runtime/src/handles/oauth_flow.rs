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
use meerkat_core::handles::{DslTransitionError, LeaseKey};

use crate::auth_machine::dsl as auth_dsl;
use crate::store::RuntimeStore;

use super::RuntimeAuthLeaseHandle;
use super::auth_lease::{AuthLeaseReleaseObserver, ReleasedOAuthFlows};

type StoreSlot = Arc<Mutex<Option<Weak<dyn RuntimeStore>>>>;
type PayloadLock = Arc<Mutex<()>>;
type RemovedBrowserSnapshotKeys = Vec<BrowserSnapshotKey>;
type RemovedDeviceSnapshotKeys = Vec<DeviceSnapshotKey>;

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

fn load_oauth_snapshot_for_release(
    store: &StoreSlot,
    operation: &'static str,
) -> Result<Option<OAuthFlowRegistrySnapshot>, DslTransitionError> {
    let store = store
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(store) = store else {
        return Ok(None);
    };
    let store = store.upgrade().ok_or_else(|| {
        DslTransitionError::new(operation, "runtime store is no longer available")
    })?;
    let Some(bytes) = store
        .load_auth_oauth_flow_snapshot()
        .map_err(|err| DslTransitionError::new(operation, err.to_string()))?
    else {
        return Ok(None);
    };
    serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&bytes)
        .map(Some)
        .map_err(|err| DslTransitionError::new(operation, err.to_string()))
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
    fn oauth_flows_for_release(
        &self,
        lease_key: &LeaseKey,
    ) -> Result<ReleasedOAuthFlows, DslTransitionError> {
        let _payload_guard = self
            .payload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let target = ConnectionRef {
            realm: lease_key.realm.clone(),
            binding: lease_key.binding.clone(),
            profile: lease_key.profile.clone(),
        };
        let Some(snapshot) =
            load_oauth_snapshot_for_release(&self.store, "collect_oauth_flow_payloads")?
        else {
            return Ok(ReleasedOAuthFlows {
                lease_key: lease_key.clone(),
                browser_flow_ids: Vec::new(),
                device_flow_ids: Vec::new(),
            });
        };
        let now_millis = current_time_millis();
        Ok(ReleasedOAuthFlows {
            lease_key: lease_key.clone(),
            browser_flow_ids: snapshot
                .browser
                .iter()
                .filter(|flow| flow.target == target && flow.expires_at_millis > now_millis)
                .map(|flow| flow.state.clone())
                .collect(),
            device_flow_ids: snapshot
                .device
                .iter()
                .filter(|flow| flow.target == target && flow.expires_at_millis > now_millis)
                .map(|flow| flow.device_code.clone())
                .collect(),
        })
    }

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
        let mut removed_browser = snapshot
            .browser
            .iter()
            .filter(|flow| flow.target == target && browser_flow_ids.contains(flow.state.as_str()))
            .map(persisted_browser_snapshot_key)
            .collect::<BTreeSet<_>>();
        let mut removed_device = snapshot
            .device
            .iter()
            .filter(|flow| {
                flow.target == target && device_flow_ids.contains(flow.device_code.as_str())
            })
            .map(persisted_device_snapshot_key)
            .collect::<BTreeSet<_>>();
        if let Some(durable) =
            load_oauth_snapshot_for_release(&self.store, "release_oauth_flow_payloads")?
        {
            removed_browser.extend(
                durable
                    .browser
                    .iter()
                    .filter(|flow| {
                        flow.target == target
                            && browser_flow_ids.contains(flow.state.as_str())
                            && flow.expires_at_millis > now_millis
                    })
                    .map(persisted_browser_snapshot_key),
            );
            removed_device.extend(
                durable
                    .device
                    .iter()
                    .filter(|flow| {
                        flow.target == target
                            && device_flow_ids.contains(flow.device_code.as_str())
                            && flow.expires_at_millis > now_millis
                    })
                    .map(persisted_device_snapshot_key),
            );
        }
        let removed_browser = removed_browser.into_iter().collect::<Vec<_>>();
        let removed_device = removed_device.into_iter().collect::<Vec<_>>();
        snapshot.browser.retain(|flow| {
            !(flow.target == target && browser_flow_ids.contains(flow.state.as_str()))
        });
        snapshot.device.retain(|flow| {
            !(flow.target == target && device_flow_ids.contains(flow.device_code.as_str()))
        });
        persist_registry_snapshot(
            &snapshot,
            &self.store,
            "release_oauth_flow_payloads",
            &removed_browser,
            &removed_device,
            now_millis,
            SnapshotPersistPolicy::merge(),
        )
        .map_err(|err| {
            DslTransitionError::new(
                "AuthLeaseReleaseObserver::release_oauth_flow_payloads",
                err.to_string(),
            )
        })?;
        let _ = self.registry.retain_flows_with_lifecycle(
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

    fn retain_registry_payloads_with_lifecycle(
        &self,
    ) -> (OAuthPrunedFlows, OAuthFlowRegistrySnapshot) {
        let before = self
            .registry
            .snapshot_for_persistence(current_time_millis());
        let pruned = self.registry.retain_flows_with_lifecycle(
            |target, flow_id| self.lifecycle.has_oauth_browser_flow(target, flow_id),
            |target, flow_id| self.lifecycle.has_oauth_device_flow(target, flow_id),
        );
        (pruned, before)
    }

    fn expire_collected_flows(&self, pruned: OAuthPrunedFlows) {
        for (flow_id, target) in pruned.browser {
            let _ = self.expire_browser(&target, &flow_id);
        }
        for (device_code, target) in pruned.device {
            let _ = self.lifecycle.expire_device_flow(&target, &device_code);
        }
    }

    fn removed_snapshot_keys_from_pruned(
        snapshot: &OAuthFlowRegistrySnapshot,
        pruned: &OAuthPrunedFlows,
    ) -> (RemovedBrowserSnapshotKeys, RemovedDeviceSnapshotKeys) {
        let pruned_browser = pruned
            .browser
            .iter()
            .map(|(flow_id, target)| browser_snapshot_key(target, flow_id))
            .collect::<BTreeSet<_>>();
        let pruned_device = pruned
            .device
            .iter()
            .map(|(device_code, target)| device_snapshot_key(target, device_code))
            .collect::<BTreeSet<_>>();
        let browser = snapshot
            .browser
            .iter()
            .filter(|flow| pruned_browser.contains(&persisted_browser_snapshot_key(flow)))
            .map(persisted_browser_snapshot_key)
            .collect();
        let device = snapshot
            .device
            .iter()
            .filter(|flow| pruned_device.contains(&persisted_device_snapshot_key(flow)))
            .map(persisted_device_snapshot_key)
            .collect();
        (browser, device)
    }

    fn store(&self) -> Option<Arc<dyn RuntimeStore>> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }

    fn persist_registry_payloads_removing(
        &self,
        operation: &'static str,
        removed_browser: &[BrowserSnapshotKey],
        removed_device: &[DeviceSnapshotKey],
    ) -> Result<(), OAuthFlowError> {
        persist_registry_payloads(
            &self.registry,
            &self.store,
            operation,
            removed_browser,
            removed_device,
        )
    }

    fn persist_registry_payloads_claiming_removal(
        &self,
        operation: &'static str,
        removed_browser: &[BrowserSnapshotKey],
        removed_device: &[DeviceSnapshotKey],
    ) -> Result<(), OAuthFlowError> {
        persist_registry_payloads_claiming_removal(
            &self.registry,
            &self.store,
            operation,
            removed_browser,
            removed_device,
        )
    }

    fn persist_registry_payloads_claiming_admission(
        &self,
        operation: &'static str,
        removed_browser: &[BrowserSnapshotKey],
        removed_device: &[DeviceSnapshotKey],
        admitted_browser: &[BrowserSnapshotKey],
        admitted_device: &[DeviceSnapshotKey],
    ) -> Result<(), OAuthFlowError> {
        persist_registry_payloads_claiming_admission(
            &self.registry,
            &self.store,
            operation,
            removed_browser,
            removed_device,
            admitted_browser,
            admitted_device,
        )
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

        for persisted in snapshot.browser.iter().cloned() {
            self.restore_browser_payload(persisted, now_millis, now_instant);
        }
        for persisted in snapshot.device.iter().cloned() {
            self.restore_device_payload(persisted, now_millis, now_instant);
        }
        let current = self.registry.snapshot_for_persistence(now_millis);
        let current_browser = current
            .browser
            .iter()
            .map(persisted_browser_snapshot_key)
            .collect::<BTreeSet<_>>();
        let current_device = current
            .device
            .iter()
            .map(persisted_device_snapshot_key)
            .collect::<BTreeSet<_>>();
        let removed_browser = snapshot
            .browser
            .iter()
            .filter(|flow| !current_browser.contains(&persisted_browser_snapshot_key(flow)))
            .map(persisted_browser_snapshot_key)
            .collect::<Vec<_>>();
        let removed_device = snapshot
            .device
            .iter()
            .filter(|flow| !current_device.contains(&persisted_device_snapshot_key(flow)))
            .map(persisted_device_snapshot_key)
            .collect::<Vec<_>>();
        let _ = self.persist_registry_payloads_removing(
            "rehydrate_oauth_flows",
            &removed_browser,
            &removed_device,
        );
    }

    fn sync_persisted_payloads(&self, operation: &'static str) -> Result<(), OAuthFlowError> {
        let Some(store) = self.store() else {
            return Ok(());
        };
        let snapshot =
            match store.load_auth_oauth_flow_snapshot().map_err(|err| {
                OAuthFlowError::PersistenceFailed {
                    operation,
                    detail: err.to_string(),
                }
            })? {
                Some(bytes) => serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&bytes)
                    .map_err(|err| OAuthFlowError::PersistenceFailed {
                        operation,
                        detail: err.to_string(),
                    })?,
                None => OAuthFlowRegistrySnapshot::default(),
            };
        let now_millis = current_time_millis();
        let now_instant = Instant::now();
        let durable_browser = snapshot
            .browser
            .iter()
            .filter(|flow| flow.expires_at_millis > now_millis)
            .map(persisted_browser_snapshot_key)
            .collect::<BTreeSet<_>>();
        let durable_device = snapshot
            .device
            .iter()
            .filter(|flow| flow.expires_at_millis > now_millis)
            .map(persisted_device_snapshot_key)
            .collect::<BTreeSet<_>>();
        let current = self.registry.snapshot_for_persistence(now_millis);
        for flow in current
            .browser
            .iter()
            .filter(|flow| !durable_browser.contains(&persisted_browser_snapshot_key(flow)))
        {
            if let Some(provider) = OAuthProviderIdentity::from_alias(&flow.provider) {
                let _ =
                    self.registry
                        .consume(&flow.state, &flow.target, provider, &flow.redirect_uri);
            }
            let _ = self.expire_browser(&flow.target, &flow.state);
        }
        for flow in current
            .device
            .iter()
            .filter(|flow| !durable_device.contains(&persisted_device_snapshot_key(flow)))
        {
            if let Some(provider) = OAuthProviderIdentity::from_alias(&flow.provider) {
                let _ = self
                    .registry
                    .expire_device_code(&flow.device_code, &flow.target, provider);
            }
            let _ = self
                .lifecycle
                .expire_device_flow(&flow.target, &flow.device_code);
        }
        let current = self.registry.snapshot_for_persistence(now_millis);
        let current_browser = current
            .browser
            .iter()
            .map(persisted_browser_snapshot_key)
            .collect::<BTreeSet<_>>();
        let current_device = current
            .device
            .iter()
            .map(persisted_device_snapshot_key)
            .collect::<BTreeSet<_>>();
        for persisted in snapshot
            .browser
            .iter()
            .filter(|flow| {
                flow.expires_at_millis > now_millis
                    && !current_browser.contains(&persisted_browser_snapshot_key(flow))
            })
            .cloned()
        {
            self.restore_browser_payload(persisted, now_millis, now_instant);
        }
        for persisted in snapshot
            .device
            .iter()
            .filter(|flow| {
                flow.expires_at_millis > now_millis
                    && !current_device.contains(&persisted_device_snapshot_key(flow))
            })
            .cloned()
        {
            self.restore_device_payload(persisted, now_millis, now_instant);
        }
        Ok(())
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
    removed_browser: &[BrowserSnapshotKey],
    removed_device: &[DeviceSnapshotKey],
) -> Result<(), OAuthFlowError> {
    let now_millis = current_time_millis();
    let snapshot = registry.snapshot_for_persistence(now_millis);
    persist_registry_snapshot(
        &snapshot,
        store,
        operation,
        removed_browser,
        removed_device,
        now_millis,
        SnapshotPersistPolicy::merge(),
    )
}

fn persist_existing_registry_payloads(
    registry: &OAuthFlowRegistry,
    store: &StoreSlot,
    operation: &'static str,
) -> Result<(), OAuthFlowError> {
    let now_millis = current_time_millis();
    let snapshot = registry.snapshot_for_persistence(now_millis);
    persist_registry_snapshot(
        &snapshot,
        store,
        operation,
        &[],
        &[],
        now_millis,
        SnapshotPersistPolicy::merge_existing(),
    )
}

fn persist_registry_payloads_claiming_removal(
    registry: &OAuthFlowRegistry,
    store: &StoreSlot,
    operation: &'static str,
    removed_browser: &[BrowserSnapshotKey],
    removed_device: &[DeviceSnapshotKey],
) -> Result<(), OAuthFlowError> {
    let now_millis = current_time_millis();
    let snapshot = registry.snapshot_for_persistence(now_millis);
    persist_registry_snapshot(
        &snapshot,
        store,
        operation,
        removed_browser,
        removed_device,
        now_millis,
        SnapshotPersistPolicy::claim_removal(),
    )
}

fn persist_registry_payloads_claiming_admission(
    registry: &OAuthFlowRegistry,
    store: &StoreSlot,
    operation: &'static str,
    removed_browser: &[BrowserSnapshotKey],
    removed_device: &[DeviceSnapshotKey],
    admitted_browser: &[BrowserSnapshotKey],
    admitted_device: &[DeviceSnapshotKey],
) -> Result<(), OAuthFlowError> {
    let now_millis = current_time_millis();
    let snapshot = registry.snapshot_for_persistence(now_millis);
    persist_registry_snapshot(
        &snapshot,
        store,
        operation,
        removed_browser,
        removed_device,
        now_millis,
        SnapshotPersistPolicy::claim_admission(
            registry.max_outstanding(),
            admitted_browser,
            admitted_device,
        ),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SnapshotRemovalMode {
    Merge,
    Claim,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SnapshotPersistPolicy<'a> {
    removal_mode: SnapshotRemovalMode,
    admission_capacity: Option<usize>,
    admitted_browser: &'a [BrowserSnapshotKey],
    admitted_device: &'a [DeviceSnapshotKey],
}

impl<'a> SnapshotPersistPolicy<'a> {
    fn merge() -> Self {
        Self {
            removal_mode: SnapshotRemovalMode::Merge,
            admission_capacity: None,
            admitted_browser: &[],
            admitted_device: &[],
        }
    }

    fn merge_existing() -> Self {
        Self::merge()
    }

    fn claim_removal() -> Self {
        Self {
            removal_mode: SnapshotRemovalMode::Claim,
            admission_capacity: None,
            admitted_browser: &[],
            admitted_device: &[],
        }
    }

    fn claim_admission(
        max_outstanding: usize,
        admitted_browser: &'a [BrowserSnapshotKey],
        admitted_device: &'a [DeviceSnapshotKey],
    ) -> Self {
        Self {
            removal_mode: SnapshotRemovalMode::Merge,
            admission_capacity: Some(max_outstanding),
            admitted_browser,
            admitted_device,
        }
    }
}

fn persist_registry_snapshot(
    snapshot: &OAuthFlowRegistrySnapshot,
    store: &StoreSlot,
    operation: &'static str,
    removed_browser: &[BrowserSnapshotKey],
    removed_device: &[DeviceSnapshotKey],
    now_millis: u64,
    policy: SnapshotPersistPolicy<'_>,
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
    let mut update = |current: Option<&[u8]>| -> Result<Vec<u8>, crate::store::RuntimeStoreError> {
        let merged = merge_oauth_registry_snapshot(
            current,
            snapshot,
            removed_browser,
            removed_device,
            now_millis,
            policy,
        )?;
        serde_json::to_vec(&merged)
            .map_err(|err| crate::store::RuntimeStoreError::WriteFailed(err.to_string()))
    };
    match store.update_auth_oauth_flow_snapshot(&mut update) {
        Ok(()) => Ok(()),
        Err(crate::store::RuntimeStoreError::NotFound(_))
            if policy.removal_mode == SnapshotRemovalMode::Claim =>
        {
            Err(OAuthFlowError::RegistryProjectionMissing { operation })
        }
        Err(crate::store::RuntimeStoreError::Internal(detail))
            if policy.admission_capacity.is_some() && detail == DURABLE_OAUTH_CAPACITY_EXCEEDED =>
        {
            Err(OAuthFlowError::CapacityExceeded {
                max_outstanding: policy.admission_capacity.unwrap_or(0),
            })
        }
        Err(err) => Err(OAuthFlowError::PersistenceFailed {
            operation,
            detail: err.to_string(),
        }),
    }
}

type BrowserSnapshotKey = (String, String, Option<String>, String);
type DeviceSnapshotKey = (String, String, Option<String>, String);
const DURABLE_OAUTH_CAPACITY_EXCEEDED: &str = "oauth durable capacity exceeded";

fn target_snapshot_key(target: &ConnectionRef) -> (String, String, Option<String>) {
    (
        target.realm.to_string(),
        target.binding.to_string(),
        target.profile.as_ref().map(ToString::to_string),
    )
}

fn browser_snapshot_key(target: &ConnectionRef, state: &str) -> BrowserSnapshotKey {
    let (realm, binding, profile) = target_snapshot_key(target);
    (realm, binding, profile, state.to_string())
}

fn device_snapshot_key(target: &ConnectionRef, device_code: &str) -> DeviceSnapshotKey {
    let (realm, binding, profile) = target_snapshot_key(target);
    (realm, binding, profile, device_code.to_string())
}

fn persisted_browser_snapshot_key(flow: &PersistedOAuthBrowserFlow) -> BrowserSnapshotKey {
    browser_snapshot_key(&flow.target, &flow.state)
}

fn persisted_device_snapshot_key(flow: &PersistedOAuthDeviceFlow) -> DeviceSnapshotKey {
    device_snapshot_key(&flow.target, &flow.device_code)
}

fn ensure_removed_flows_are_active(
    current: &OAuthFlowRegistrySnapshot,
    removed_browser: &BTreeSet<BrowserSnapshotKey>,
    removed_device: &BTreeSet<DeviceSnapshotKey>,
    now_millis: u64,
) -> Result<(), crate::store::RuntimeStoreError> {
    let active_browser = current
        .browser
        .iter()
        .filter(|flow| flow.expires_at_millis > now_millis)
        .map(persisted_browser_snapshot_key)
        .collect::<BTreeSet<_>>();
    for key in removed_browser {
        if !active_browser.contains(key) {
            return Err(crate::store::RuntimeStoreError::NotFound(
                "oauth browser flow was already consumed".to_string(),
            ));
        }
    }

    let active_device = current
        .device
        .iter()
        .filter(|flow| flow.expires_at_millis > now_millis)
        .map(persisted_device_snapshot_key)
        .collect::<BTreeSet<_>>();
    for key in removed_device {
        if !active_device.contains(key) {
            return Err(crate::store::RuntimeStoreError::NotFound(
                "oauth device flow was already consumed".to_string(),
            ));
        }
    }
    Ok(())
}

fn ensure_merged_snapshot_within_capacity(
    merged: &OAuthFlowRegistrySnapshot,
    max_outstanding: usize,
) -> Result<(), crate::store::RuntimeStoreError> {
    if merged.browser.len().saturating_add(merged.device.len()) > max_outstanding {
        return Err(crate::store::RuntimeStoreError::Internal(
            DURABLE_OAUTH_CAPACITY_EXCEEDED.to_string(),
        ));
    }
    Ok(())
}

fn merge_oauth_registry_snapshot(
    current: Option<&[u8]>,
    local: &OAuthFlowRegistrySnapshot,
    removed_browser: &[BrowserSnapshotKey],
    removed_device: &[DeviceSnapshotKey],
    now_millis: u64,
    policy: SnapshotPersistPolicy<'_>,
) -> Result<OAuthFlowRegistrySnapshot, crate::store::RuntimeStoreError> {
    let mut merged = match current {
        Some(bytes) => serde_json::from_slice::<OAuthFlowRegistrySnapshot>(bytes)
            .map_err(|err| crate::store::RuntimeStoreError::WriteFailed(err.to_string()))?,
        None => OAuthFlowRegistrySnapshot::default(),
    };
    let removed_browser = removed_browser.iter().cloned().collect::<BTreeSet<_>>();
    let removed_device = removed_device.iter().cloned().collect::<BTreeSet<_>>();
    let admitted_browser = policy
        .admitted_browser
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let admitted_device = policy
        .admitted_device
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if policy.removal_mode == SnapshotRemovalMode::Claim {
        ensure_removed_flows_are_active(&merged, &removed_browser, &removed_device, now_millis)?;
    }
    let current_browser = merged
        .browser
        .iter()
        .filter(|flow| flow.expires_at_millis > now_millis)
        .map(persisted_browser_snapshot_key)
        .collect::<BTreeSet<_>>();
    let current_device = merged
        .device
        .iter()
        .filter(|flow| flow.expires_at_millis > now_millis)
        .map(persisted_device_snapshot_key)
        .collect::<BTreeSet<_>>();
    let local_browser = local
        .browser
        .iter()
        .map(persisted_browser_snapshot_key)
        .collect::<BTreeSet<_>>();
    let local_device = local
        .device
        .iter()
        .map(persisted_device_snapshot_key)
        .collect::<BTreeSet<_>>();

    merged.browser.retain(|flow| {
        flow.expires_at_millis > now_millis
            && !removed_browser.contains(&persisted_browser_snapshot_key(flow))
            && !local_browser.contains(&persisted_browser_snapshot_key(flow))
    });
    merged.device.retain(|flow| {
        flow.expires_at_millis > now_millis
            && !removed_device.contains(&persisted_device_snapshot_key(flow))
            && !local_device.contains(&persisted_device_snapshot_key(flow))
    });
    merged.browser.extend(
        local
            .browser
            .iter()
            .filter(|flow| {
                let key = persisted_browser_snapshot_key(flow);
                flow.expires_at_millis > now_millis
                    && !removed_browser.contains(&key)
                    && (current_browser.contains(&key) || admitted_browser.contains(&key))
            })
            .cloned(),
    );
    merged.device.extend(
        local
            .device
            .iter()
            .filter(|flow| {
                let key = persisted_device_snapshot_key(flow);
                flow.expires_at_millis > now_millis
                    && !removed_device.contains(&key)
                    && (current_device.contains(&key) || admitted_device.contains(&key))
            })
            .cloned(),
    );
    merged.browser.sort_by_key(persisted_browser_snapshot_key);
    merged.device.sort_by_key(persisted_device_snapshot_key);
    if let Some(max_outstanding) = policy.admission_capacity {
        ensure_merged_snapshot_within_capacity(&merged, max_outstanding)?;
    }
    Ok(merged)
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
    fn device_flow_state_is_authmachine_owned(&self) -> bool {
        true
    }

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
    fn device_flow_state_is_authmachine_owned(&self) -> bool {
        true
    }

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
        persist_existing_registry_payloads(
            &self.registry,
            &self.store,
            "persist_oauth_device_flow_payloads",
        )
    }

    fn device_flow_payload_removed(
        &self,
        record: &OAuthDeviceFlowRecord,
    ) -> Result<(), OAuthFlowError> {
        let removed = [device_snapshot_key(&record.target, &record.device_code)];
        persist_registry_payloads_claiming_removal(
            &self.registry,
            &self.store,
            "consume_oauth_device_flow",
            &[],
            &removed,
        )
    }
}

impl OAuthFlowAuthority for RuntimeOAuthFlowHandle {
    fn terminal_flow_state_is_authmachine_owned(&self) -> bool {
        true
    }

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
        self.sync_persisted_payloads("admit_oauth_browser_flow")?;
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
        let mut lifecycle_pruned = OAuthPrunedFlows::default();
        let mut lifecycle_pruned_snapshot = None;
        if matches!(inserted, Err(OAuthFlowError::CapacityExceeded { .. })) {
            let (pruned, snapshot) = self.retain_registry_payloads_with_lifecycle();
            lifecycle_pruned = pruned;
            lifecycle_pruned_snapshot = Some(snapshot);
            inserted = self.registry.insert_browser_flow_with_pruned(
                state.clone(),
                target.clone(),
                provider,
                redirect_uri.clone(),
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
        let (removed_browser, removed_device) =
            if let Some(snapshot) = lifecycle_pruned_snapshot.as_ref() {
                Self::removed_snapshot_keys_from_pruned(snapshot, &lifecycle_pruned)
            } else {
                (Vec::new(), Vec::new())
            };
        self.expire_collected_flows(pruned);
        let admitted_browser = [browser_snapshot_key(&target, &state)];
        if let Err(err) = self.persist_registry_payloads_claiming_admission(
            "admit_oauth_browser_flow",
            &removed_browser,
            &removed_device,
            &admitted_browser,
            &[],
        ) {
            let _ = self
                .registry
                .consume(&state, &target, provider, &redirect_uri);
            let _ = self.expire_browser(&target, &state);
            return Err(err);
        }
        Ok(state)
    }

    fn verify(
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
        self.sync_persisted_payloads("verify_oauth_browser_flow")?;
        self.expire_pruned_flows();
        self.verify_browser(target, state, provider, redirect_uri)?;
        match self.registry.verify(state, target, provider, redirect_uri) {
            Ok(record) => Ok(record),
            Err(OAuthFlowError::Missing) => Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "verify_oauth_browser_flow",
            }),
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
        self.sync_persisted_payloads("consume_oauth_browser_flow")?;
        self.expire_pruned_flows();
        self.verify_browser(target, state, provider, redirect_uri)?;
        let record = match self.registry.verify(state, target, provider, redirect_uri) {
            Ok(record) => record,
            Err(OAuthFlowError::Missing) => {
                return Err(OAuthFlowError::RegistryProjectionMissing {
                    operation: "consume_oauth_browser_flow",
                });
            }
            Err(err) => return Err(err),
        };
        self.consume_browser(target, state, provider, redirect_uri)?;
        if let Err(err) = self.registry.consume(state, target, provider, redirect_uri) {
            let _ = self.restore_browser_flow(state, &record);
            return Err(match err {
                OAuthFlowError::Missing => OAuthFlowError::RegistryProjectionMissing {
                    operation: "consume_oauth_browser_flow",
                },
                other => other,
            });
        }
        let removed_browser = [browser_snapshot_key(target, state)];
        if let Err(err) = self.persist_registry_payloads_claiming_removal(
            "consume_oauth_browser_flow",
            &removed_browser,
            &[],
        ) {
            if matches!(
                err,
                OAuthFlowError::Missing | OAuthFlowError::RegistryProjectionMissing { .. }
            ) {
                return Err(err);
            }
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
        self.sync_persisted_payloads("admit_oauth_device_flow")?;
        self.expire_pruned_flows();
        let machine_expires_at = expires_at_millis(expires_in)?;
        self.admit_device(&target, &device_code, provider, machine_expires_at)?;
        let mut inserted = self.registry.admit_device_code_with_pruned(
            target.clone(),
            provider,
            device_code.clone(),
            expires_in,
        );
        let mut lifecycle_pruned = OAuthPrunedFlows::default();
        let mut lifecycle_pruned_snapshot = None;
        if matches!(inserted, Err(OAuthFlowError::CapacityExceeded { .. })) {
            let (pruned, snapshot) = self.retain_registry_payloads_with_lifecycle();
            lifecycle_pruned = pruned;
            lifecycle_pruned_snapshot = Some(snapshot);
            inserted = self.registry.admit_device_code_with_pruned(
                target.clone(),
                provider,
                device_code.clone(),
                expires_in,
            );
        }
        let pruned = match inserted {
            Ok(pruned) => pruned,
            Err(err) => {
                let _ = self.lifecycle.expire_device_flow(&target, &device_code);
                return Err(err);
            }
        };
        let (removed_browser, removed_device) =
            if let Some(snapshot) = lifecycle_pruned_snapshot.as_ref() {
                Self::removed_snapshot_keys_from_pruned(snapshot, &lifecycle_pruned)
            } else {
                (Vec::new(), Vec::new())
            };
        self.expire_collected_flows(pruned);
        let admitted_device = [device_snapshot_key(&target, &device_code)];
        if let Err(err) = self.persist_registry_payloads_claiming_admission(
            "admit_oauth_device_flow",
            &removed_browser,
            &removed_device,
            &[],
            &admitted_device,
        ) {
            let _ = self
                .registry
                .expire_device_code(&device_code, &target, provider);
            let _ = self.lifecycle.expire_device_flow(&target, &device_code);
            return Err(err);
        }
        Ok(())
    }

    fn verify_device_code(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
        let _payload_guard = self
            .payload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.sync_persisted_payloads("verify_oauth_device_flow")?;
        self.expire_pruned_flows();
        self.verify_device(target, device_code, provider)?;
        match self
            .registry
            .verify_device_code(device_code, target, provider)
        {
            Ok(record) => Ok(record),
            Err(OAuthFlowError::Missing) => Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "verify_oauth_device_flow",
            }),
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
        self.sync_persisted_payloads("begin_oauth_device_poll")?;
        self.expire_pruned_flows();
        self.begin_device_poll(target, device_code, provider)?;
        let poll = match self
            .registry
            .begin_device_code_poll(device_code, target, provider)
        {
            Ok(poll) => poll,
            Err(OAuthFlowError::Missing) => {
                let _ = self.lifecycle.finish_device_poll(target, device_code);
                return Err(OAuthFlowError::RegistryProjectionMissing {
                    operation: "begin_oauth_device_poll",
                });
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

        fn update_auth_oauth_flow_snapshot(
            &self,
            update: &mut crate::store::AuthOAuthFlowSnapshotUpdate<'_>,
        ) -> Result<(), RuntimeStoreError> {
            self.wait_if_oauth_persist_blocked();
            if self.fail_oauth_persist.load(Ordering::SeqCst) {
                return Err(RuntimeStoreError::WriteFailed(
                    "injected oauth snapshot failure".to_string(),
                ));
            }
            let mut snapshot = self
                .snapshot
                .lock()
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            let next = update(snapshot.as_deref())?;
            *snapshot = Some(next);
            Ok(())
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
    fn missing_browser_projection_cannot_overwrite_authmachine_flow() {
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

        authority
            .registry
            .consume(&state, &target, provider, redirect_uri)
            .expect("test removes only the local registry payload");

        assert!(matches!(
            authority.verify(&state, &target, provider, redirect_uri),
            Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "verify_oauth_browser_flow"
            })
        ));
        assert!(
            lifecycle.has_oauth_browser_flow_for_test(&target, &state),
            "missing process-local registry payload must not expire canonical AuthMachine membership"
        );

        assert!(matches!(
            authority.consume(&state, &target, provider, redirect_uri),
            Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "consume_oauth_browser_flow"
            })
        ));
        assert!(
            lifecycle.has_oauth_browser_flow_for_test(&target, &state),
            "terminal consume must fail closed instead of converting payload loss to not-found"
        );
        assert_eq!(
            snapshot_phase(&lifecycle, &target),
            Some(AuthLeasePhase::ReauthRequired)
        );
    }

    #[test]
    fn missing_device_poll_projection_cannot_overwrite_authmachine_flow() {
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
        let poll = authority
            .begin_device_code_poll(device_code, &target, provider)
            .expect("device poll begins");
        authority
            .registry
            .expire_device_code(device_code, &target, provider)
            .expect("test removes only the local registry payload");

        assert!(matches!(
            poll.consume(),
            Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "consume_oauth_device_flow"
            })
        ));
        assert!(
            lifecycle.has_oauth_device_flow_for_test(&target, device_code),
            "missing process-local poll payload must not expire canonical AuthMachine membership"
        );
    }

    #[test]
    fn browser_admit_persistence_failure_rolls_back_unreturned_flow() {
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            lifecycle.clone(),
            &store_dyn,
        );
        let failed_target = target();
        let successful_target = alternate_target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";

        store.fail_oauth_persist();
        assert!(matches!(
            authority.start(
                failed_target.clone(),
                provider,
                redirect_uri.to_string(),
                "failed-verifier".to_string(),
            ),
            Err(OAuthFlowError::PersistenceFailed { .. })
        ));
        assert!(
            authority
                .registry
                .snapshot_for_persistence(current_time_millis())
                .browser
                .is_empty(),
            "failed browser admission must not leave an unreturned registry payload"
        );

        store.allow_oauth_persist();
        let successful_state = authority
            .start(
                successful_target,
                provider,
                redirect_uri.to_string(),
                "successful-verifier".to_string(),
            )
            .expect("subsequent browser admit persists after store recovers");
        let snapshot_json = store
            .load_auth_oauth_flow_snapshot()
            .expect("durable OAuth snapshot loads")
            .expect("durable OAuth snapshot exists");
        let snapshot = serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&snapshot_json)
            .expect("durable OAuth snapshot decodes");
        assert_eq!(
            snapshot
                .browser
                .iter()
                .map(|flow| flow.state.as_str())
                .collect::<Vec<_>>(),
            vec![successful_state.as_str()],
            "a later successful admit must not persist a previously failed unreturned flow"
        );
    }

    #[test]
    fn device_admit_persistence_failure_rolls_back_unreturned_flow() {
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
        let failed_device_code = "failed-device-code";
        let successful_device_code = "successful-device-code";

        store.fail_oauth_persist();
        assert!(matches!(
            authority.admit_device_code(
                target.clone(),
                provider,
                failed_device_code.to_string(),
                Duration::from_secs(60),
            ),
            Err(OAuthFlowError::PersistenceFailed { .. })
        ));
        assert!(matches!(
            authority
                .registry
                .verify_device_code(failed_device_code, &target, provider),
            Err(OAuthFlowError::Missing)
        ));
        assert!(!lifecycle.has_oauth_device_flow_for_test(&target, failed_device_code));

        store.allow_oauth_persist();
        authority
            .admit_device_code(
                target,
                provider,
                successful_device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("subsequent device admit persists after store recovers");
        let snapshot_json = store
            .load_auth_oauth_flow_snapshot()
            .expect("durable OAuth snapshot loads")
            .expect("durable OAuth snapshot exists");
        let snapshot = serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&snapshot_json)
            .expect("durable OAuth snapshot decodes");
        assert_eq!(
            snapshot
                .device
                .iter()
                .map(|flow| flow.device_code.as_str())
                .collect::<Vec<_>>(),
            vec![successful_device_code],
            "a later successful admit must not persist a previously failed unreturned device flow"
        );
    }

    #[cfg(feature = "sqlite-store")]
    #[test]
    fn persistent_oauth_snapshot_merges_independent_authority_writes() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let store_path = temp_dir.path().join("runtime.sqlite");
        let store_one: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let store_two: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let first_authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_one,
        );
        let second_authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_two,
        );
        let first_target = target();
        let second_target = alternate_target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;

        let first_state = first_authority
            .start(
                first_target.clone(),
                provider,
                "http://127.0.0.1/callback".to_string(),
                "verifier-1".to_string(),
            )
            .expect("first process admits browser flow");
        let second_state = second_authority
            .start(
                second_target.clone(),
                provider,
                "http://127.0.0.1/other-callback".to_string(),
                "verifier-2".to_string(),
            )
            .expect("second process admits browser flow");

        let store_three: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let snapshot_json = store_three
            .load_auth_oauth_flow_snapshot()
            .expect("durable OAuth snapshot loads")
            .expect("durable OAuth snapshot exists");
        let snapshot = serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&snapshot_json)
            .expect("durable OAuth snapshot decodes");
        assert!(
            snapshot
                .browser
                .iter()
                .any(|flow| flow.state == first_state),
            "the first independent authority write must survive the second write"
        );
        assert!(
            snapshot
                .browser
                .iter()
                .any(|flow| flow.state == second_state),
            "the second independent authority write must be persisted"
        );

        let restarted = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_three,
        );
        restarted
            .consume(
                &first_state,
                &first_target,
                provider,
                "http://127.0.0.1/callback",
            )
            .expect("first independent flow rehydrates");
        restarted
            .consume(
                &second_state,
                &second_target,
                provider,
                "http://127.0.0.1/other-callback",
            )
            .expect("second independent flow rehydrates after first consume");
    }

    #[test]
    fn persistent_oauth_browser_admit_does_not_resurrect_consumed_between_sync_and_persist() {
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let creator = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        let target = target();
        let replacement_target = alternate_target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";
        let replacement_redirect_uri = "http://127.0.0.1/replacement-callback";
        let consumed_state = creator
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "consumed-verifier".to_string(),
            )
            .expect("creator admits browser flow");

        let stale_authority = Arc::new(
            RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
                Duration::from_secs(60),
                Arc::new(RuntimeAuthLeaseHandle::new()),
                &store_dyn,
            ),
        );
        let consumer = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );

        store.block_next_oauth_persist();
        let stale_admit = std::thread::spawn({
            let stale_authority = Arc::clone(&stale_authority);
            let replacement_target = replacement_target.clone();
            move || {
                stale_authority.start(
                    replacement_target,
                    provider,
                    replacement_redirect_uri.to_string(),
                    "replacement-verifier".to_string(),
                )
            }
        });
        store.wait_for_blocked_oauth_persist();
        consumer
            .consume(&consumed_state, &target, provider, redirect_uri)
            .expect("independent authority consumes browser flow between sync and persist");
        store.release_blocked_oauth_persist();
        let replacement_state = stale_admit
            .join()
            .expect("stale browser admit thread should not panic")
            .expect("stale authority admits replacement browser flow");

        let snapshot_json = store
            .load_auth_oauth_flow_snapshot()
            .expect("durable OAuth snapshot loads")
            .expect("durable OAuth snapshot exists");
        let snapshot = serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&snapshot_json)
            .expect("durable OAuth snapshot decodes");
        assert!(
            !snapshot
                .browser
                .iter()
                .any(|flow| flow.state == consumed_state),
            "a stale admission must not resurrect a browser flow consumed after pre-sync"
        );
        assert!(
            snapshot
                .browser
                .iter()
                .any(|flow| flow.state == replacement_state),
            "the stale authority's newly admitted browser flow should still persist"
        );

        let restarted = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        assert!(matches!(
            restarted.consume(&consumed_state, &target, provider, redirect_uri),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "verify_oauth_browser_flow",
                ..
            })
        ));
        restarted
            .consume(
                &replacement_state,
                &replacement_target,
                provider,
                replacement_redirect_uri,
            )
            .expect("new stale-authority flow survives restart");
    }

    #[test]
    fn persistent_oauth_device_admit_does_not_resurrect_consumed_between_sync_and_persist() {
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let creator = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        let target = target();
        let replacement_target = alternate_target();
        let provider = OAuthProviderIdentity::GoogleCodeAssist;
        let consumed_device_code = "consumed-device-code";
        let replacement_device_code = "replacement-device-code";
        creator
            .admit_device_code(
                target.clone(),
                provider,
                consumed_device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("creator admits device flow");

        let stale_authority = Arc::new(
            RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
                Duration::from_secs(60),
                Arc::new(RuntimeAuthLeaseHandle::new()),
                &store_dyn,
            ),
        );
        let consumer = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );

        store.block_next_oauth_persist();
        let stale_admit = std::thread::spawn({
            let stale_authority = Arc::clone(&stale_authority);
            let replacement_target = replacement_target.clone();
            move || {
                stale_authority.admit_device_code(
                    replacement_target,
                    provider,
                    replacement_device_code.to_string(),
                    Duration::from_secs(60),
                )
            }
        });
        store.wait_for_blocked_oauth_persist();
        consumer
            .begin_device_code_poll(consumed_device_code, &target, provider)
            .expect("independent authority begins device poll")
            .consume()
            .expect("independent authority consumes device flow between sync and persist");
        store.release_blocked_oauth_persist();
        stale_admit
            .join()
            .expect("stale device admit thread should not panic")
            .expect("stale authority admits replacement device flow");

        let snapshot_json = store
            .load_auth_oauth_flow_snapshot()
            .expect("durable OAuth snapshot loads")
            .expect("durable OAuth snapshot exists");
        let snapshot = serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&snapshot_json)
            .expect("durable OAuth snapshot decodes");
        assert!(
            !snapshot
                .device
                .iter()
                .any(|flow| flow.device_code == consumed_device_code),
            "a stale admission must not resurrect a device flow consumed after pre-sync"
        );
        assert!(
            snapshot
                .device
                .iter()
                .any(|flow| flow.device_code == replacement_device_code),
            "the stale authority's newly admitted device flow should still persist"
        );

        let restarted = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        assert!(matches!(
            restarted.verify_device_code(consumed_device_code, &target, provider),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "verify_oauth_device_flow",
                ..
            })
        ));
        restarted
            .verify_device_code(replacement_device_code, &replacement_target, provider)
            .expect("new stale-authority device flow survives restart");
    }

    #[cfg(feature = "sqlite-store")]
    #[test]
    fn persistent_oauth_device_poll_finish_does_not_resurrect_consumed_payload() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let store_path = temp_dir.path().join("runtime.sqlite");
        let creator_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let stale_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let consumer_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let creator = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &creator_store,
        );
        let target = target();
        let provider = OAuthProviderIdentity::GoogleCodeAssist;
        let device_code = "pending-finish-device-code";
        creator
            .admit_device_code(
                target.clone(),
                provider,
                device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("creator admits device flow");

        let stale_authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &stale_store,
        );
        let stale_poll = stale_authority
            .begin_device_code_poll(device_code, &target, provider)
            .expect("stale authority begins pending poll");
        let consumer = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &consumer_store,
        );
        consumer
            .begin_device_code_poll(device_code, &target, provider)
            .expect("consumer begins independent poll")
            .consume()
            .expect("consumer consumes durable payload");

        stale_poll
            .finish()
            .expect("stale pending poll finish is local cleanup only");

        let snapshot_json = stale_store
            .load_auth_oauth_flow_snapshot()
            .expect("durable OAuth snapshot loads")
            .expect("durable OAuth snapshot exists");
        let snapshot = serde_json::from_slice::<OAuthFlowRegistrySnapshot>(&snapshot_json)
            .expect("durable OAuth snapshot decodes");
        assert!(
            !snapshot
                .device
                .iter()
                .any(|flow| flow.device_code == device_code),
            "a stale pending poll finish must not resurrect a consumed device flow"
        );

        let restarted_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let restarted = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &restarted_store,
        );
        assert!(matches!(
            restarted.verify_device_code(device_code, &target, provider),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "verify_oauth_device_flow",
                ..
            })
        ));
    }

    #[cfg(feature = "sqlite-store")]
    #[test]
    fn persistent_oauth_browser_sync_prunes_stale_capacity_before_admit() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let store_path = temp_dir.path().join("runtime.sqlite");
        let creator_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let stale_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let consumer_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let creator = RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
            Duration::from_secs(60),
            1,
            Arc::new(RuntimeAuthLeaseHandle::new()),
            Some(Arc::downgrade(&creator_store)),
        );
        let target = target();
        let replacement_target = alternate_target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";
        let replacement_redirect_uri = "http://127.0.0.1/replacement-callback";
        let consumed_state = creator
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "consumed-verifier".to_string(),
            )
            .expect("creator admits browser flow");

        let stale_authority = RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
            Duration::from_secs(60),
            1,
            Arc::new(RuntimeAuthLeaseHandle::new()),
            Some(Arc::downgrade(&stale_store)),
        );
        let consumer = RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
            Duration::from_secs(60),
            1,
            Arc::new(RuntimeAuthLeaseHandle::new()),
            Some(Arc::downgrade(&consumer_store)),
        );
        consumer
            .consume(&consumed_state, &target, provider, redirect_uri)
            .expect("independent authority consumes browser flow");

        let replacement_state = stale_authority
            .start(
                replacement_target.clone(),
                provider,
                replacement_redirect_uri.to_string(),
                "replacement-verifier".to_string(),
            )
            .expect("stale capacity is pruned before browser admit");
        stale_authority
            .consume(
                &replacement_state,
                &replacement_target,
                provider,
                replacement_redirect_uri,
            )
            .expect("replacement browser flow remains usable");
    }

    #[cfg(feature = "sqlite-store")]
    #[test]
    fn persistent_oauth_device_sync_prunes_stale_capacity_before_admit() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let store_path = temp_dir.path().join("runtime.sqlite");
        let creator_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let stale_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let consumer_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let creator = RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
            Duration::from_secs(60),
            1,
            Arc::new(RuntimeAuthLeaseHandle::new()),
            Some(Arc::downgrade(&creator_store)),
        );
        let target = target();
        let replacement_target = alternate_target();
        let provider = OAuthProviderIdentity::GoogleCodeAssist;
        let consumed_device_code = "consumed-capacity-device-code";
        let replacement_device_code = "replacement-device-code";
        creator
            .admit_device_code(
                target.clone(),
                provider,
                consumed_device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("creator admits device flow");

        let stale_authority = RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
            Duration::from_secs(60),
            1,
            Arc::new(RuntimeAuthLeaseHandle::new()),
            Some(Arc::downgrade(&stale_store)),
        );
        let consumer = RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
            Duration::from_secs(60),
            1,
            Arc::new(RuntimeAuthLeaseHandle::new()),
            Some(Arc::downgrade(&consumer_store)),
        );
        consumer
            .begin_device_code_poll(consumed_device_code, &target, provider)
            .expect("independent authority begins device poll")
            .consume()
            .expect("independent authority consumes device flow");

        stale_authority
            .admit_device_code(
                replacement_target.clone(),
                provider,
                replacement_device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("stale capacity is pruned before device admit");
        stale_authority
            .verify_device_code(replacement_device_code, &replacement_target, provider)
            .expect("replacement device flow remains usable");
    }

    #[test]
    fn concurrent_persistent_browser_consumes_require_fresh_durable_claim() {
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let creator = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        let target = target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";
        let state = creator
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "verifier".to_string(),
            )
            .expect("browser flow admitted");
        let first = Arc::new(
            RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
                Duration::from_secs(60),
                Arc::new(RuntimeAuthLeaseHandle::new()),
                &store_dyn,
            ),
        );
        let second = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );

        store.block_next_oauth_persist();
        let first_consume = std::thread::spawn({
            let first = Arc::clone(&first);
            let state = state.clone();
            let target = target.clone();
            move || first.consume(&state, &target, provider, redirect_uri)
        });
        store.wait_for_blocked_oauth_persist();

        second
            .consume(&state, &target, provider, redirect_uri)
            .expect("second authority wins durable consume race");
        store.release_blocked_oauth_persist();
        assert!(matches!(
            first_consume
                .join()
                .expect("first consume thread should not panic"),
            Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "consume_oauth_browser_flow"
            })
        ));
    }

    #[test]
    fn concurrent_persistent_device_consumes_require_fresh_durable_claim() {
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let creator = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        let target = target();
        let provider = OAuthProviderIdentity::GoogleCodeAssist;
        let device_code = "race-device-code";
        creator
            .admit_device_code(
                target.clone(),
                provider,
                device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("device flow admitted");
        let first = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        let second = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        let first_poll = first
            .begin_device_code_poll(device_code, &target, provider)
            .expect("first authority begins poll");
        let second_poll = second
            .begin_device_code_poll(device_code, &target, provider)
            .expect("second authority begins poll");

        store.block_next_oauth_persist();
        let first_consume = std::thread::spawn(move || first_poll.consume());
        store.wait_for_blocked_oauth_persist();

        second_poll
            .consume()
            .expect("second authority wins durable device consume race");
        store.release_blocked_oauth_persist();
        assert!(matches!(
            first_consume
                .join()
                .expect("first device consume thread should not panic"),
            Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "consume_oauth_device_flow"
            })
        ));
    }

    #[test]
    fn concurrent_persistent_browser_admits_require_fresh_durable_capacity() {
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let first_store = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let second_store = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let first = Arc::new(
            RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
                Duration::from_secs(60),
                1,
                Arc::new(RuntimeAuthLeaseHandle::new()),
                Some(Arc::downgrade(&first_store)),
            ),
        );
        let second = RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
            Duration::from_secs(60),
            1,
            Arc::new(RuntimeAuthLeaseHandle::new()),
            Some(Arc::downgrade(&second_store)),
        );
        let first_target = target();
        let second_target = alternate_target();
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let first_redirect_uri = "http://127.0.0.1/first-callback";
        let second_redirect_uri = "http://127.0.0.1/second-callback";

        store.block_next_oauth_persist();
        let first_admit = std::thread::spawn({
            let first = Arc::clone(&first);
            let first_target = first_target.clone();
            move || {
                first.start(
                    first_target,
                    provider,
                    first_redirect_uri.to_string(),
                    "first-verifier".to_string(),
                )
            }
        });
        store.wait_for_blocked_oauth_persist();

        second
            .start(
                second_target,
                provider,
                second_redirect_uri.to_string(),
                "second-verifier".to_string(),
            )
            .expect("second authority wins durable browser admission race");
        store.release_blocked_oauth_persist();
        assert!(matches!(
            first_admit
                .join()
                .expect("first browser admit thread should not panic"),
            Err(OAuthFlowError::CapacityExceeded { max_outstanding: 1 })
        ));
    }

    #[test]
    fn concurrent_persistent_device_admits_require_fresh_durable_capacity() {
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let first_store = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let second_store = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let first = Arc::new(
            RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
                Duration::from_secs(60),
                1,
                Arc::new(RuntimeAuthLeaseHandle::new()),
                Some(Arc::downgrade(&first_store)),
            ),
        );
        let second = RuntimeOAuthFlowHandle::new_with_capacity_auth_lease_and_store(
            Duration::from_secs(60),
            1,
            Arc::new(RuntimeAuthLeaseHandle::new()),
            Some(Arc::downgrade(&second_store)),
        );
        let first_target = target();
        let second_target = alternate_target();
        let provider = OAuthProviderIdentity::GoogleCodeAssist;

        store.block_next_oauth_persist();
        let first_admit = std::thread::spawn({
            let first = Arc::clone(&first);
            let first_target = first_target.clone();
            move || {
                first.admit_device_code(
                    first_target,
                    provider,
                    "first-device-code".to_string(),
                    Duration::from_secs(60),
                )
            }
        });
        store.wait_for_blocked_oauth_persist();

        second
            .admit_device_code(
                second_target,
                provider,
                "second-device-code".to_string(),
                Duration::from_secs(60),
            )
            .expect("second authority wins durable device admission race");
        store.release_blocked_oauth_persist();
        assert!(matches!(
            first_admit
                .join()
                .expect("first device admit thread should not panic"),
            Err(OAuthFlowError::CapacityExceeded { max_outstanding: 1 })
        ));
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
    fn stale_release_persistence_failure_does_not_install_released_authority() {
        let releasing_lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let store = Arc::new(FailingOAuthSnapshotStore::default());
        let store_dyn = Arc::clone(&store) as Arc<dyn RuntimeStore>;
        let releasing_authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            releasing_lifecycle.clone(),
            &store_dyn,
        );
        let admitting_authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &store_dyn,
        );
        let target = target();
        let lease_key = LeaseKey::from_connection_ref(&target);
        let provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";
        let state = admitting_authority
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "verifier".to_string(),
            )
            .expect("other authority admits browser flow");
        assert!(
            !releasing_lifecycle.has_oauth_browser_flow_for_test(&target, &state),
            "releasing authority starts stale and has no local machine membership"
        );

        store.fail_oauth_persist();
        assert!(
            releasing_lifecycle.release_lease(&lease_key).is_err(),
            "release should fail closed when stale durable OAuth cleanup cannot persist"
        );
        assert_eq!(
            releasing_lifecycle.snapshot(&lease_key).phase,
            None,
            "failed stale release must not synthesize a local AuthMachine authority"
        );

        store.allow_oauth_persist();
        releasing_authority
            .consume(&state, &target, provider, redirect_uri)
            .expect("failed stale durable release must leave browser flow retryable");
    }

    #[cfg(feature = "sqlite-store")]
    #[test]
    fn persistent_release_prunes_durable_flows_from_stale_authority() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let store_path = temp_dir.path().join("runtime.sqlite");
        let releasing_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let admitting_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let releasing_lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let releasing_authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            releasing_lifecycle.clone(),
            &releasing_store,
        );
        let admitting_authority = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &admitting_store,
        );
        let target = target();
        let lease_key = LeaseKey::from_connection_ref(&target);
        let browser_provider = OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1/callback";
        let browser_state = admitting_authority
            .start(
                target.clone(),
                browser_provider,
                redirect_uri.to_string(),
                "browser-verifier".to_string(),
            )
            .expect("other authority admits browser flow");
        let device_provider = OAuthProviderIdentity::GoogleCodeAssist;
        let device_code = "released-device-code";
        admitting_authority
            .admit_device_code(
                target.clone(),
                device_provider,
                device_code.to_string(),
                Duration::from_secs(60),
            )
            .expect("other authority admits device flow");
        assert!(
            !releasing_lifecycle.has_oauth_browser_flow_for_test(&target, &browser_state),
            "releasing authority starts stale and does not know the browser flow locally"
        );
        assert!(
            !releasing_lifecycle.has_oauth_device_flow_for_test(&target, device_code),
            "releasing authority starts stale and does not know the device flow locally"
        );

        releasing_lifecycle
            .release_lease(&lease_key)
            .expect("stale release succeeds");

        let restarted_store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::sqlite::SqliteRuntimeStore::new(&store_path).unwrap());
        let restarted = RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_secs(60),
            Arc::new(RuntimeAuthLeaseHandle::new()),
            &restarted_store,
        );
        let browser_after_release =
            restarted.consume(&browser_state, &target, browser_provider, redirect_uri);
        let device_after_release =
            restarted.verify_device_code(device_code, &target, device_provider);
        assert!(
            matches!(
                browser_after_release,
                Err(OAuthFlowError::LifecycleRejected {
                    operation: "verify_oauth_browser_flow",
                    ..
                })
            ),
            "release from a stale authority must prune durable browser flow, got {browser_after_release:?}"
        );
        assert!(
            matches!(
                device_after_release,
                Err(OAuthFlowError::LifecycleRejected {
                    operation: "verify_oauth_device_flow",
                    ..
                })
            ),
            "release from a stale authority must prune durable device flow, got {device_after_release:?}"
        );
        drop(releasing_authority);
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
    fn device_admit_rejects_registry_pruned_canonical_membership() {
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

        assert!(matches!(
            authority.admit_device_code(
                target.clone(),
                provider,
                device_code.to_string(),
                Duration::from_secs(60),
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "admit_oauth_device_flow",
                ..
            })
        ));

        assert!(
            lifecycle.has_oauth_device_flow_for_test(&target, device_code),
            "registry-only loss must not expire canonical AuthMachine device membership"
        );
        assert!(matches!(
            authority.verify_device_code(device_code, &target, provider),
            Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "verify_oauth_device_flow"
            })
        ));
        assert!(matches!(
            authority.begin_device_code_poll(device_code, &target, provider),
            Err(OAuthFlowError::RegistryProjectionMissing {
                operation: "begin_oauth_device_poll"
            })
        ));
        assert!(
            lifecycle.has_oauth_device_flow_for_test(&target, device_code),
            "missing process-local device payload must fail closed without removing the flow"
        );
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

        assert!(matches!(
            duplicate,
            Err(OAuthFlowError::LifecycleRejected {
                operation: "admit_oauth_device_flow",
                ..
            })
        ));
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
        let _hook_guard = crate::handles::auth_lease::install_release_after_accept_hook_for_test(
            Arc::new(move |released_key| {
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
            }),
        );

        lifecycle
            .release_lease(&lease_key)
            .expect("credential lifecycle release succeeds");

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
            Err(OAuthFlowError::LifecycleRejected {
                operation: "verify_oauth_browser_flow",
                ..
            })
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
