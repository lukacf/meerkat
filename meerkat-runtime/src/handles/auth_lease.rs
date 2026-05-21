//! Runtime impl of [`meerkat_core::handles::AuthLeaseHandle`].
//!
//! Per-binding [`AuthMachine`](crate::auth_machine) instances, keyed by
//! typed [`LeaseKey`](meerkat_core::handles::LeaseKey). Each trait
//! method looks up or creates the corresponding machine, then fires
//! the matching DSL input. `snapshot` projects the machine's phase
//! + expires_at back out.
//!
//! The original Phase 1.5-rev design absorbed auth state into
//! MeerkatMachine as Sets+Maps keyed by binding. Post-review that was
//! rejected: auth lifecycle is orthogonal to MeerkatMachine's
//! lifecycle, and carrying it there grew the TLC state space without
//! unifying any semantics (dogma §19). Splitting it out per-binding
//! keeps each machine small, aligns the fact "this binding's lease
//! state" with one canonical owner per dogma §1, and decouples auth
//! evolution from core-runtime evolution.

use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Weak;
use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::AuthBindingRef;
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseRestoreSnapshot, AuthLeaseSnapshot,
    AuthLeaseTransition, DslTransitionError, LeaseKey,
};
use meerkat_core::time_compat::{SystemTime, UNIX_EPOCH};

use crate::auth_machine::dsl as auth_dsl;

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Emit a structured audit record for every accepted auth-lease DSL
/// transition. REST/RPC surfaces (and any other `tracing::Subscriber`
/// consumer) can filter on `target = "meerkat::auth::audit"` to build
/// a persistent audit log without the shell inventing the fact set
/// (dogma §17 — surfaces observe, they do not own lifecycle truth).
///
/// Deferral §5: structured observability for auth events. We emit at
/// the single choke point where DSL transitions succeed, not from each
/// caller — that keeps the audit trail aligned with the machine's
/// actual acceptance of the transition and avoids duplication /
/// drift (dogma §1).
fn emit_audit(
    lease_key: &LeaseKey,
    action: &'static str,
    from_phase: AuthLeasePhase,
    to_phase: AuthLeasePhase,
) {
    tracing::info!(
        target: "meerkat::auth::audit",
        lease_key = %lease_key,
        realm = %lease_key.realm,
        binding = %lease_key.binding,
        profile = lease_key.profile.as_ref().map(meerkat_core::ProfileId::as_str),
        action = %action,
        from_phase = ?from_phase,
        to_phase = ?to_phase,
        "auth lease transition"
    );
}

/// Runtime-backed [`AuthLeaseHandle`] impl.
///
/// Holds a mutex-guarded registry of per-binding [`auth_dsl::AuthMachineAuthority`]
/// instances. Lookup-or-insert happens on first `acquire_lease`; release is
/// also allowed before acquire so token-clear surfaces remain idempotent after
/// process restart.
#[derive(Clone)]
pub struct RuntimeAuthLeaseHandle {
    machines: Arc<Mutex<AuthLeaseRegistry>>,
    #[cfg(not(target_arch = "wasm32"))]
    release_observers: Arc<Mutex<Vec<Weak<dyn AuthLeaseReleaseObserver>>>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub(crate) struct ReleasedOAuthFlows {
    pub lease_key: LeaseKey,
    pub browser_flow_ids: Vec<String>,
    pub device_flow_ids: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ReleasedOAuthFlows {
    fn empty(lease_key: LeaseKey) -> Self {
        Self {
            lease_key,
            browser_flow_ids: Vec::new(),
            device_flow_ids: Vec::new(),
        }
    }

    fn dedup(&mut self) {
        self.browser_flow_ids.sort();
        self.browser_flow_ids.dedup();
        self.device_flow_ids.sort();
        self.device_flow_ids.dedup();
    }
}

fn has_oauth_membership(state: &auth_dsl::AuthMachineState) -> bool {
    !state.oauth_browser_flow_ids.is_empty()
        || !state.oauth_device_flow_ids.is_empty()
        || !state.oauth_device_poll_ids.is_empty()
}

#[cfg(not(target_arch = "wasm32"))]
fn merge_oauth_membership(
    restored: &mut auth_dsl::AuthMachineState,
    current: &auth_dsl::AuthMachineState,
) {
    for flow_id in &current.oauth_browser_flow_ids {
        restored.oauth_browser_flow_ids.insert(flow_id.clone());
        if let Some(provider) = current.oauth_browser_flow_providers.get(flow_id).cloned() {
            restored
                .oauth_browser_flow_providers
                .insert(flow_id.clone(), provider);
        }
        if let Some(redirect_uri) = current
            .oauth_browser_flow_redirect_uris
            .get(flow_id)
            .cloned()
        {
            restored
                .oauth_browser_flow_redirect_uris
                .insert(flow_id.clone(), redirect_uri);
        }
        if let Some(expires_at_millis) = current
            .oauth_browser_flow_expires_at_millis
            .get(flow_id)
            .copied()
        {
            restored
                .oauth_browser_flow_expires_at_millis
                .insert(flow_id.clone(), expires_at_millis);
        }
    }
    for flow_id in &current.oauth_device_flow_ids {
        restored.oauth_device_flow_ids.insert(flow_id.clone());
        if let Some(provider) = current.oauth_device_flow_providers.get(flow_id).cloned() {
            restored
                .oauth_device_flow_providers
                .insert(flow_id.clone(), provider);
        }
        if let Some(expires_at_millis) = current
            .oauth_device_flow_expires_at_millis
            .get(flow_id)
            .copied()
        {
            restored
                .oauth_device_flow_expires_at_millis
                .insert(flow_id.clone(), expires_at_millis);
        }
    }
    for poll_id in &current.oauth_device_poll_ids {
        restored.oauth_device_poll_ids.insert(poll_id.clone());
    }
    let browser_count = u64::try_from(restored.oauth_browser_flow_ids.len()).unwrap_or(u64::MAX);
    let device_count = u64::try_from(restored.oauth_device_flow_ids.len()).unwrap_or(u64::MAX);
    restored.oauth_outstanding_flow_count = browser_count.saturating_add(device_count);
}

fn restore_input_from_state(state: &auth_dsl::AuthMachineState) -> auth_dsl::AuthMachineInput {
    restore_input_from_lifecycle(
        state.lifecycle_phase,
        state.expires_at,
        state.last_refresh,
        state.refresh_attempt,
        state.credential_present,
        state.credential_generation,
        state.credential_published_at_millis,
    )
}

fn restore_input_from_lifecycle(
    lifecycle_phase: auth_dsl::AuthLifecyclePhase,
    expires_at: Option<u64>,
    last_refresh: Option<u64>,
    refresh_attempt: u64,
    credential_present: bool,
    credential_generation: u64,
    credential_published_at_millis: Option<u64>,
) -> auth_dsl::AuthMachineInput {
    auth_dsl::AuthMachineInput::RestoreAuthoritySnapshot {
        lifecycle_phase,
        expires_at,
        last_refresh,
        refresh_attempt,
        credential_present,
        credential_generation,
        credential_published_at_millis,
    }
}

fn restore_oauth_inputs_from_state(
    state: &auth_dsl::AuthMachineState,
    context: &'static str,
) -> Result<Vec<auth_dsl::AuthMachineInput>, DslTransitionError> {
    let mut inputs = Vec::new();
    for flow_id in &state.oauth_browser_flow_ids {
        let provider = state
            .oauth_browser_flow_providers
            .get(flow_id)
            .cloned()
            .ok_or_else(|| {
                DslTransitionError::guard_rejected(
                    context,
                    format!("OAuth browser flow `{flow_id}` missing provider in restore snapshot"),
                )
            })?;
        let redirect_uri = state
            .oauth_browser_flow_redirect_uris
            .get(flow_id)
            .cloned()
            .ok_or_else(|| {
                DslTransitionError::guard_rejected(
                    context,
                    format!(
                        "OAuth browser flow `{flow_id}` missing redirect URI in restore snapshot"
                    ),
                )
            })?;
        let expires_at_millis = state
            .oauth_browser_flow_expires_at_millis
            .get(flow_id)
            .copied()
            .ok_or_else(|| {
                DslTransitionError::guard_rejected(
                    context,
                    format!("OAuth browser flow `{flow_id}` missing expiry in restore snapshot"),
                )
            })?;
        inputs.push(auth_dsl::AuthMachineInput::RestoreOAuthBrowserFlow {
            flow_id: flow_id.clone(),
            provider,
            redirect_uri,
            expires_at_millis,
        });
    }
    for flow_id in &state.oauth_device_flow_ids {
        let provider = state
            .oauth_device_flow_providers
            .get(flow_id)
            .cloned()
            .ok_or_else(|| {
                DslTransitionError::guard_rejected(
                    context,
                    format!("OAuth device flow `{flow_id}` missing provider in restore snapshot"),
                )
            })?;
        let expires_at_millis = state
            .oauth_device_flow_expires_at_millis
            .get(flow_id)
            .copied()
            .ok_or_else(|| {
                DslTransitionError::guard_rejected(
                    context,
                    format!("OAuth device flow `{flow_id}` missing expiry in restore snapshot"),
                )
            })?;
        inputs.push(auth_dsl::AuthMachineInput::RestoreOAuthDeviceFlow {
            flow_id: flow_id.clone(),
            provider,
            expires_at_millis,
            poll_active: state.oauth_device_poll_ids.contains(flow_id),
        });
    }
    for poll_id in &state.oauth_device_poll_ids {
        if !state.oauth_device_flow_ids.contains(poll_id) {
            return Err(DslTransitionError::guard_rejected(
                context,
                format!("OAuth device poll `{poll_id}` has no device flow in restore snapshot"),
            ));
        }
    }
    Ok(inputs)
}

fn map_auth_machine_error(
    err: auth_dsl::AuthMachineTransitionError,
    context: &'static str,
) -> DslTransitionError {
    let reason = err.to_string();
    match err {
        auth_dsl::AuthMachineTransitionError::GuardRejected { .. } => {
            DslTransitionError::guard_rejected(context, reason)
        }
        auth_dsl::AuthMachineTransitionError::NoMatchingTransition { .. } => {
            DslTransitionError::no_matching(context, reason)
        }
    }
}

fn apply_restore_input(
    authority: &mut auth_dsl::AuthMachineAuthority,
    lease_key: &LeaseKey,
    input: auth_dsl::AuthMachineInput,
    context: &'static str,
) -> Result<(AuthLeasePhase, AuthLeaseTransition), DslTransitionError> {
    let transition = auth_dsl::AuthMachineMutator::apply(authority, input)
        .map_err(|err| map_auth_machine_error(err, context))?;
    let auth_transition = auth_lease_transition_from_generated_publication(
        lease_key,
        authority,
        &transition,
        context,
    )?;
    Ok((
        map_phase(authority.state().lifecycle_phase),
        auth_transition,
    ))
}

fn apply_restore_input_to_registry(
    registry: &mut AuthLeaseRegistry,
    lease_key: &LeaseKey,
    input: auth_dsl::AuthMachineInput,
    context: &'static str,
) -> Result<(AuthLeasePhase, AuthLeaseTransition), DslTransitionError> {
    let authority = registry
        .authorities
        .entry(lease_key.clone())
        .or_insert_with(auth_dsl::AuthMachineAuthority::new);
    apply_restore_input(authority, lease_key, input, context)
}

fn restore_state_to_registry(
    registry: &mut AuthLeaseRegistry,
    lease_key: &LeaseKey,
    state: &auth_dsl::AuthMachineState,
    context: &'static str,
) -> Result<(AuthLeasePhase, AuthLeaseTransition), DslTransitionError> {
    if state.lifecycle_phase == auth_dsl::AuthLifecyclePhase::Released
        && has_oauth_membership(state)
    {
        return Err(DslTransitionError::guard_rejected(
            context,
            "released AuthMachine restore snapshot cannot carry OAuth membership",
        ));
    }
    let oauth_inputs = restore_oauth_inputs_from_state(state, context)?;
    let restored = apply_restore_input_to_registry(
        registry,
        lease_key,
        restore_input_from_state(state),
        context,
    )?;
    for input in oauth_inputs {
        apply_restore_input_to_registry(registry, lease_key, input, context)?;
    }
    Ok(restored)
}

fn restore_lifecycle_with_oauth_to_registry(
    registry: &mut AuthLeaseRegistry,
    lease_key: &LeaseKey,
    oauth: &auth_dsl::AuthMachineState,
    lifecycle_phase: auth_dsl::AuthLifecyclePhase,
    expires_at: Option<u64>,
    last_refresh: Option<u64>,
    refresh_attempt: u64,
    credential_present: bool,
    credential_generation: u64,
    credential_published_at_millis: Option<u64>,
    context: &'static str,
) -> Result<(AuthLeasePhase, AuthLeaseTransition), DslTransitionError> {
    let oauth_inputs = restore_oauth_inputs_from_state(oauth, context)?;
    let restored = apply_restore_input_to_registry(
        registry,
        lease_key,
        restore_input_from_lifecycle(
            lifecycle_phase,
            expires_at,
            last_refresh,
            refresh_attempt,
            credential_present,
            credential_generation,
            credential_published_at_millis,
        ),
        context,
    )?;
    for input in oauth_inputs {
        apply_restore_input_to_registry(registry, lease_key, input, context)?;
    }
    Ok(restored)
}

fn auth_lease_transition_from_generated_publication(
    lease_key: &LeaseKey,
    authority: &auth_dsl::AuthMachineAuthority,
    transition: &auth_dsl::AuthMachineTransition,
    context: &'static str,
) -> Result<AuthLeaseTransition, DslTransitionError> {
    let mut obligations =
        crate::protocol_auth_lease_lifecycle_publication::extract_obligations(transition);
    if obligations.len() != 1 {
        return Err(DslTransitionError::new(
            context,
            format!(
                "AuthMachine transition emitted {} lifecycle publication obligations",
                obligations.len()
            ),
        ));
    }
    let scope =
        crate::protocol_auth_lease_lifecycle_publication::AuthLeaseLifecyclePublicationScope::from_authority(
            lease_key.clone(),
            authority,
        );
    obligations
        .remove(0)
        .into_auth_lease_transition(scope)
        .map_err(|err| {
            DslTransitionError::new(
                context,
                format!("AuthMachine lifecycle publication handoff failed: {err}"),
            )
        })
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) trait AuthLeaseReleaseObserver: Send + Sync {
    fn oauth_flows_for_release(
        &self,
        lease_key: &LeaseKey,
    ) -> Result<ReleasedOAuthFlows, DslTransitionError> {
        Ok(ReleasedOAuthFlows::empty(lease_key.clone()))
    }

    fn auth_lease_released(&self, released: &ReleasedOAuthFlows) -> Result<(), DslTransitionError>;
}

#[cfg(test)]
pub(crate) type ReleaseAfterAcceptHook = Arc<dyn Fn(&LeaseKey) + Send + Sync>;

#[cfg(test)]
static RELEASE_AFTER_ACCEPT_HOOK: std::sync::OnceLock<Mutex<Option<ReleaseAfterAcceptHook>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
static RELEASE_AFTER_ACCEPT_HOOK_SERIAL: std::sync::OnceLock<Mutex<()>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) struct ReleaseAfterAcceptHookGuard {
    _serial: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for ReleaseAfterAcceptHookGuard {
    fn drop(&mut self) {
        set_release_after_accept_hook_for_test(None);
    }
}

#[cfg(test)]
pub(crate) fn install_release_after_accept_hook_for_test(
    hook: ReleaseAfterAcceptHook,
) -> ReleaseAfterAcceptHookGuard {
    let serial = RELEASE_AFTER_ACCEPT_HOOK_SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    set_release_after_accept_hook_for_test(Some(hook));
    ReleaseAfterAcceptHookGuard { _serial: serial }
}

#[cfg(test)]
fn set_release_after_accept_hook_for_test(hook: Option<ReleaseAfterAcceptHook>) {
    *RELEASE_AFTER_ACCEPT_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
}

#[cfg(test)]
fn run_release_after_accept_hook(lease_key: &LeaseKey) {
    let hook = RELEASE_AFTER_ACCEPT_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(hook) = hook {
        hook(lease_key);
    }
}

#[derive(Default)]
struct AuthLeaseRegistry {
    authorities: HashMap<LeaseKey, auth_dsl::AuthMachineAuthority>,
}

impl std::fmt::Debug for RuntimeAuthLeaseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.debug_struct("RuntimeAuthLeaseHandle")
            .field("leases", &guard.authorities.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RuntimeAuthLeaseHandle {
    pub fn new() -> Self {
        Self {
            machines: Arc::new(Mutex::new(AuthLeaseRegistry::default())),
            #[cfg(not(target_arch = "wasm32"))]
            release_observers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Alias for [`Self::new`]; kept for parity with other runtime
    /// handles that distinguish session-owned vs. ephemeral
    /// constructors. AuthMachine instances are always ephemeral from
    /// the registry's perspective — the registry itself is owned by
    /// the session bindings.
    pub fn ephemeral() -> Self {
        Self::new()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn add_release_observer(&self, observer: Weak<dyn AuthLeaseReleaseObserver>) {
        self.release_observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(observer);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn live_release_observers(&self) -> Vec<Arc<dyn AuthLeaseReleaseObserver>> {
        {
            let mut guard = self
                .release_observers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut observers = Vec::new();
            guard.retain(|observer| match observer.upgrade() {
                Some(observer) => {
                    observers.push(observer);
                    true
                }
                None => false,
            });
            observers
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn collect_release_observer_flows(
        &self,
        observers: &[Arc<dyn AuthLeaseReleaseObserver>],
        lease_key: &LeaseKey,
    ) -> Result<ReleasedOAuthFlows, DslTransitionError> {
        let mut released = ReleasedOAuthFlows::empty(lease_key.clone());
        for observer in observers {
            let mut observed = observer.oauth_flows_for_release(lease_key)?;
            released
                .browser_flow_ids
                .append(&mut observed.browser_flow_ids);
            released
                .device_flow_ids
                .append(&mut observed.device_flow_ids);
        }
        released.dedup();
        Ok(released)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn notify_release_observers(
        &self,
        observers: &[Arc<dyn AuthLeaseReleaseObserver>],
        released: &ReleasedOAuthFlows,
    ) -> Result<(), DslTransitionError> {
        for observer in observers {
            observer.auth_lease_released(released)?;
        }
        Ok(())
    }

    fn apply(
        &self,
        lease_key: &LeaseKey,
        input: auth_dsl::AuthMachineInput,
        context: &'static str,
        create_if_missing: bool,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let action = Self::audit_action_for(&input);
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !create_if_missing && !guard.authorities.contains_key(lease_key) {
            return Err(DslTransitionError::new(
                context,
                format!("no auth lease registered for lease_key `{lease_key}`"),
            ));
        }
        let (from_phase, to_phase, auth_transition) = {
            let entry = if create_if_missing {
                guard
                    .authorities
                    .entry(lease_key.clone())
                    .or_insert_with(auth_dsl::AuthMachineAuthority::new)
            } else {
                match guard.authorities.get_mut(lease_key) {
                    Some(m) => m,
                    None => {
                        return Err(DslTransitionError::new(
                            context,
                            format!("no auth lease registered for lease_key `{lease_key}`"),
                        ));
                    }
                }
            };
            let from_phase = map_phase(entry.state().lifecycle_phase);
            let transition = auth_dsl::AuthMachineMutator::apply(entry, input)
                .map_err(|err| map_auth_machine_error(err, context))?;
            let auth_transition = auth_lease_transition_from_generated_publication(
                lease_key,
                entry,
                &transition,
                context,
            )?;
            let to_phase = map_phase(entry.state().lifecycle_phase);
            (from_phase, to_phase, auth_transition)
        };
        emit_audit(lease_key, action, from_phase, to_phase);
        Ok(auth_transition)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn oauth_global_outstanding_flow_count(registry: &AuthLeaseRegistry) -> u64 {
        registry
            .authorities
            .values()
            .map(|authority| authority.state().oauth_outstanding_flow_count)
            .fold(0u64, u64::saturating_add)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn attach_oauth_global_observation(
        input: auth_dsl::AuthMachineInput,
        observed_global_outstanding_flows: u64,
    ) -> auth_dsl::AuthMachineInput {
        match input {
            auth_dsl::AuthMachineInput::AdmitOAuthBrowserFlow {
                flow_id,
                provider,
                redirect_uri,
                expires_at_millis,
                max_outstanding_flows,
                ..
            } => auth_dsl::AuthMachineInput::AdmitOAuthBrowserFlow {
                flow_id,
                provider,
                redirect_uri,
                expires_at_millis,
                max_outstanding_flows,
                observed_global_outstanding_flows,
            },
            auth_dsl::AuthMachineInput::AdmitOAuthDeviceFlow {
                flow_id,
                provider,
                expires_at_millis,
                max_outstanding_flows,
                ..
            } => auth_dsl::AuthMachineInput::AdmitOAuthDeviceFlow {
                flow_id,
                provider,
                expires_at_millis,
                max_outstanding_flows,
                observed_global_outstanding_flows,
            },
            other => other,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn apply_oauth_input(
        &self,
        target: &AuthBindingRef,
        input: auth_dsl::AuthMachineInput,
        context: &'static str,
        create_if_missing: bool,
    ) -> Result<(), DslTransitionError> {
        let lease_key = LeaseKey::from_auth_binding(target);
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let input = Self::attach_oauth_global_observation(
            input,
            Self::oauth_global_outstanding_flow_count(&guard),
        );
        let action = Self::audit_action_for(&input);
        if create_if_missing && !guard.authorities.contains_key(&lease_key) {
            let mut authority = auth_dsl::AuthMachineAuthority::new();
            auth_dsl::AuthMachineMutator::apply(
                &mut authority,
                auth_dsl::AuthMachineInput::MarkReauthRequired,
            )
            .map_err(|err| map_auth_machine_error(err, context))?;
            guard.authorities.insert(lease_key.clone(), authority);
        }
        let (from_phase, to_phase) = {
            let entry = match guard.authorities.get_mut(&lease_key) {
                Some(m) => m,
                None => {
                    return Err(DslTransitionError::new(
                        context,
                        format!("no auth machine registered for lease_key `{lease_key}`"),
                    ));
                }
            };
            let from_phase = map_phase(entry.state().lifecycle_phase);
            auth_dsl::AuthMachineMutator::apply(entry, input)
                .map_err(|err| map_auth_machine_error(err, context))?;
            let to_phase = map_phase(entry.state().lifecycle_phase);
            (from_phase, to_phase)
        };
        emit_audit(&lease_key, action, from_phase, to_phase);
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn confirm_oauth_durable_admission(
        &self,
        target: &AuthBindingRef,
        observed_global_outstanding_flows: u64,
        max_outstanding_flows: u64,
        context: &'static str,
    ) -> Result<(), DslTransitionError> {
        let lease_key = LeaseKey::from_auth_binding(target);
        let input = auth_dsl::AuthMachineInput::ConfirmOAuthDurableAdmission {
            observed_global_outstanding_flows,
            max_outstanding_flows,
        };
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (from_phase, to_phase) = {
            let entry = match guard.authorities.get_mut(&lease_key) {
                Some(m) => m,
                None => {
                    return Err(DslTransitionError::new(
                        context,
                        format!("no auth machine registered for lease_key `{lease_key}`"),
                    ));
                }
            };
            let from_phase = map_phase(entry.state().lifecycle_phase);
            auth_dsl::AuthMachineMutator::apply(entry, input)
                .map_err(|err| map_auth_machine_error(err, context))?;
            let to_phase = map_phase(entry.state().lifecycle_phase);
            (from_phase, to_phase)
        };
        emit_audit(
            &lease_key,
            "confirm_oauth_durable_admission",
            from_phase,
            to_phase,
        );
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn has_oauth_browser_flow(&self, target: &AuthBindingRef, flow_id: &str) -> bool {
        let lease_key = LeaseKey::from_auth_binding(target);
        self.machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .authorities
            .get(&lease_key)
            .is_some_and(|authority| authority.state().oauth_browser_flow_ids.contains(flow_id))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn has_oauth_device_flow(&self, target: &AuthBindingRef, flow_id: &str) -> bool {
        let lease_key = LeaseKey::from_auth_binding(target);
        self.machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .authorities
            .get(&lease_key)
            .is_some_and(|authority| authority.state().oauth_device_flow_ids.contains(flow_id))
    }

    #[cfg(test)]
    pub(crate) fn has_oauth_browser_flow_for_test(
        &self,
        target: &AuthBindingRef,
        flow_id: &str,
    ) -> bool {
        self.has_oauth_browser_flow(target, flow_id)
    }

    #[cfg(test)]
    pub(crate) fn has_oauth_device_flow_for_test(
        &self,
        target: &AuthBindingRef,
        flow_id: &str,
    ) -> bool {
        self.has_oauth_device_flow(target, flow_id)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn restore_released_lease_after_observer_failure(
        &self,
        lease_key: &LeaseKey,
        previous_state: Option<auth_dsl::AuthMachineState>,
    ) {
        const CONTEXT: &str = "AuthLeaseHandle::rollback_release_lease";
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current_generation = guard
            .authorities
            .get(lease_key)
            .map(|authority| authority.state().credential_generation)
            .unwrap_or(0);
        let previous_generation_value = previous_state
            .as_ref()
            .map(|state| state.credential_generation)
            .unwrap_or(0);
        if current_generation > previous_generation_value {
            let to_phase = match previous_state {
                Some(previous_state) => {
                    if let Some(current) = guard.authorities.get(lease_key) {
                        let mut current_state = current.state().clone();
                        merge_oauth_membership(&mut current_state, &previous_state);
                        restore_state_to_registry(&mut guard, lease_key, &current_state, CONTEXT)
                        .map(|(phase, _)| phase)
                        .unwrap_or_else(|err| {
                            tracing::error!(%lease_key, error = %err, "failed to restore AuthMachine after release observer failure");
                            AuthLeasePhase::Released
                        })
                    } else if has_oauth_membership(&previous_state) {
                        restore_lifecycle_with_oauth_to_registry(
                            &mut guard,
                            lease_key,
                            &previous_state,
                            auth_dsl::AuthLifecyclePhase::ReauthRequired,
                            None,
                            None,
                            0,
                            false,
                            previous_state.credential_generation,
                            None,
                            CONTEXT,
                        )
                        .map(|(phase, _)| phase)
                        .unwrap_or_else(|err| {
                            tracing::error!(%lease_key, error = %err, "failed to restore AuthMachine OAuth membership after release observer failure");
                            AuthLeasePhase::Released
                        })
                    } else {
                        AuthLeasePhase::Released
                    }
                }
                None => guard
                    .authorities
                    .get(lease_key)
                    .map(|current| map_phase(current.state().lifecycle_phase))
                    .unwrap_or(AuthLeasePhase::Released),
            };
            drop(guard);
            emit_audit(
                lease_key,
                "rollback_release_lease",
                AuthLeasePhase::Released,
                to_phase,
            );
            return;
        }
        let to_phase = if let Some(mut restored_state) = previous_state {
            if let Some(current) = guard.authorities.get(lease_key) {
                if current.state().credential_present {
                    let mut current_state = current.state().clone();
                    merge_oauth_membership(&mut current_state, &restored_state);
                    let to_phase = restore_state_to_registry(
                        &mut guard,
                        lease_key,
                        &current_state,
                        CONTEXT,
                    )
                    .map(|(phase, _)| phase)
                    .unwrap_or_else(|err| {
                        tracing::error!(%lease_key, error = %err, "failed to restore current AuthMachine credential after release observer failure");
                        AuthLeasePhase::Released
                    });
                    drop(guard);
                    emit_audit(
                        lease_key,
                        "rollback_release_lease",
                        AuthLeasePhase::Released,
                        to_phase,
                    );
                    return;
                }
                merge_oauth_membership(&mut restored_state, current.state());
            }
            let to_phase = restore_state_to_registry(&mut guard, lease_key, &restored_state, CONTEXT)
            .map(|(phase, _)| phase)
            .unwrap_or_else(|err| {
                tracing::error!(%lease_key, error = %err, "failed to restore previous AuthMachine state after release observer failure");
                AuthLeasePhase::Released
            });
            to_phase
        } else if let Some(current) = guard.authorities.get(lease_key) {
            let state = current.state();
            if state.lifecycle_phase != auth_dsl::AuthLifecyclePhase::Released
                || state.credential_present
                || has_oauth_membership(state)
            {
                map_phase(state.lifecycle_phase)
            } else {
                AuthLeasePhase::Released
            }
        } else {
            AuthLeasePhase::Released
        };
        drop(guard);
        emit_audit(
            lease_key,
            "rollback_release_lease",
            AuthLeasePhase::Released,
            to_phase,
        );
    }

    fn audit_action_for(input: &auth_dsl::AuthMachineInput) -> &'static str {
        match input {
            auth_dsl::AuthMachineInput::Acquire { .. } => "acquire_lease",
            auth_dsl::AuthMachineInput::MarkExpiring => "mark_expiring",
            auth_dsl::AuthMachineInput::BeginRefresh => "begin_refresh",
            auth_dsl::AuthMachineInput::CompleteRefresh { .. } => "complete_refresh",
            auth_dsl::AuthMachineInput::RefreshFailedTransient => "refresh_failed_transient",
            auth_dsl::AuthMachineInput::RefreshFailedPermanent => "refresh_failed_permanent",
            auth_dsl::AuthMachineInput::MarkReauthRequired => "mark_reauth_required",
            auth_dsl::AuthMachineInput::ClearCredentialLifecycle => "clear_credential_lifecycle",
            auth_dsl::AuthMachineInput::Release => "release_lease",
            auth_dsl::AuthMachineInput::RestoreAuthoritySnapshot { .. } => {
                "restore_authority_snapshot"
            }
            auth_dsl::AuthMachineInput::RestoreOAuthBrowserFlow { .. } => {
                "restore_oauth_browser_flow"
            }
            auth_dsl::AuthMachineInput::RestoreOAuthDeviceFlow { .. } => {
                "restore_oauth_device_flow"
            }
            auth_dsl::AuthMachineInput::AdmitOAuthBrowserFlow { .. } => "admit_oauth_browser_flow",
            auth_dsl::AuthMachineInput::VerifyOAuthBrowserFlow { .. } => {
                "verify_oauth_browser_flow"
            }
            auth_dsl::AuthMachineInput::ConsumeOAuthBrowserFlow { .. } => {
                "consume_oauth_browser_flow"
            }
            auth_dsl::AuthMachineInput::ExpireOAuthBrowserFlow { .. } => {
                "expire_oauth_browser_flow"
            }
            auth_dsl::AuthMachineInput::AdmitOAuthDeviceFlow { .. } => "admit_oauth_device_flow",
            auth_dsl::AuthMachineInput::ConfirmOAuthDurableAdmission { .. } => {
                "confirm_oauth_durable_admission"
            }
            auth_dsl::AuthMachineInput::VerifyOAuthDeviceFlow { .. } => "verify_oauth_device_flow",
            auth_dsl::AuthMachineInput::BeginOAuthDevicePoll { .. } => "begin_oauth_device_poll",
            auth_dsl::AuthMachineInput::FinishOAuthDevicePoll { .. } => "finish_oauth_device_poll",
            auth_dsl::AuthMachineInput::ConsumeOAuthDeviceFlow { .. } => {
                "consume_oauth_device_flow"
            }
            auth_dsl::AuthMachineInput::ExpireOAuthDeviceFlow { .. } => "expire_oauth_device_flow",
        }
    }
}

/// Exhaustive 1-to-1 projection of the AuthMachine DSL's typed lifecycle
/// phase into the cross-crate [`AuthLeasePhase`] contract. Compiler enforces
/// completeness.
fn map_phase(phase: auth_dsl::AuthLifecyclePhase) -> AuthLeasePhase {
    match phase {
        auth_dsl::AuthLifecyclePhase::Valid => AuthLeasePhase::Valid,
        auth_dsl::AuthLifecyclePhase::Expiring => AuthLeasePhase::Expiring,
        auth_dsl::AuthLifecyclePhase::Refreshing => AuthLeasePhase::Refreshing,
        auth_dsl::AuthLifecyclePhase::ReauthRequired => AuthLeasePhase::ReauthRequired,
        auth_dsl::AuthLifecyclePhase::Released => AuthLeasePhase::Released,
    }
}

fn restore_phase(phase: AuthLeasePhase) -> auth_dsl::AuthLifecyclePhase {
    match phase {
        AuthLeasePhase::Valid => auth_dsl::AuthLifecyclePhase::Valid,
        AuthLeasePhase::Expiring => auth_dsl::AuthLifecyclePhase::Expiring,
        AuthLeasePhase::Refreshing => auth_dsl::AuthLifecyclePhase::Refreshing,
        AuthLeasePhase::ReauthRequired => auth_dsl::AuthLifecyclePhase::ReauthRequired,
        AuthLeasePhase::Released => auth_dsl::AuthLifecyclePhase::Released,
    }
}

impl Default for RuntimeAuthLeaseHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthLeaseHandle for RuntimeAuthLeaseHandle {
    fn acquire_lease(
        &self,
        lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let expires_at_ts = if expires_at == u64::MAX {
            None
        } else {
            Some(expires_at)
        };
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::Acquire {
                expires_at_ts,
                credential_published_at_millis: current_time_millis(),
            },
            "AuthLeaseHandle::acquire_lease",
            true,
        )
    }

    fn mark_expiring(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::MarkExpiring,
            "AuthLeaseHandle::mark_expiring",
            false,
        )
        .map(|_| ())
    }

    fn begin_refresh(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::BeginRefresh,
            "AuthLeaseHandle::begin_refresh",
            false,
        )
        .map(|_| ())
    }

    fn complete_refresh(
        &self,
        lease_key: &LeaseKey,
        new_expires_at: u64,
        now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let new_expires_at = if new_expires_at == u64::MAX {
            None
        } else {
            Some(new_expires_at)
        };
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::CompleteRefresh {
                new_expires_at,
                now_ts: now,
                credential_published_at_millis: current_time_millis(),
            },
            "AuthLeaseHandle::complete_refresh",
            false,
        )
    }

    fn refresh_failed(
        &self,
        lease_key: &LeaseKey,
        permanent: bool,
    ) -> Result<(), DslTransitionError> {
        let input = if permanent {
            auth_dsl::AuthMachineInput::RefreshFailedPermanent
        } else {
            auth_dsl::AuthMachineInput::RefreshFailedTransient
        };
        self.apply(lease_key, input, "AuthLeaseHandle::refresh_failed", false)
            .map(|_| ())
    }

    fn mark_reauth_required(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::MarkReauthRequired,
            "AuthLeaseHandle::mark_reauth_required",
            false,
        )
        .map(|_| ())
    }

    fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let context = "AuthLeaseHandle::release_lease";
        // Capture OAuth membership while the released machine is still the
        // canonical AuthMachine state. Observers later prune only those exact
        // payloads, so a same-target flow admitted after this transition is
        // not removed by stale target-wide cleanup.
        #[cfg(not(target_arch = "wasm32"))]
        let release_observers = self.live_release_observers();
        #[cfg(not(target_arch = "wasm32"))]
        let mut released = self.collect_release_observer_flows(&release_observers, lease_key)?;
        #[cfg(not(target_arch = "wasm32"))]
        let previous_state;
        let (from_phase, to_phase) = {
            let mut guard = self
                .machines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            #[cfg(not(target_arch = "wasm32"))]
            {
                previous_state = guard
                    .authorities
                    .get(lease_key)
                    .map(|authority| authority.state().clone());
            }
            let entry = guard
                .authorities
                .entry(lease_key.clone())
                .or_insert_with(auth_dsl::AuthMachineAuthority::new);
            #[cfg(not(target_arch = "wasm32"))]
            {
                released
                    .browser_flow_ids
                    .extend(entry.state().oauth_browser_flow_ids.iter().cloned());
                released
                    .device_flow_ids
                    .extend(entry.state().oauth_device_flow_ids.iter().cloned());
                released.dedup();
            }
            let from_phase = map_phase(entry.state().lifecycle_phase);
            auth_dsl::AuthMachineMutator::apply(entry, auth_dsl::AuthMachineInput::Release)
                .map_err(|err| map_auth_machine_error(err, context))?;
            let to_phase = map_phase(entry.state().lifecycle_phase);
            (from_phase, to_phase)
        };
        emit_audit(lease_key, "release_lease", from_phase, to_phase);
        #[cfg(test)]
        run_release_after_accept_hook(lease_key);
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(err) = self.notify_release_observers(&release_observers, &released) {
            self.restore_released_lease_after_observer_failure(lease_key, previous_state);
            return Err(err);
        }
        Ok(())
    }

    fn release_credential_lifecycle(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let context = "AuthLeaseHandle::release_credential_lifecycle";
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (input, from_phase, to_phase) = {
            let entry = guard
                .authorities
                .entry(lease_key.clone())
                .or_insert_with(auth_dsl::AuthMachineAuthority::new);
            let has_oauth_membership = has_oauth_membership(entry.state());
            let input = if has_oauth_membership {
                auth_dsl::AuthMachineInput::ClearCredentialLifecycle
            } else {
                auth_dsl::AuthMachineInput::Release
            };
            let from_phase = map_phase(entry.state().lifecycle_phase);
            auth_dsl::AuthMachineMutator::apply(entry, input.clone())
                .map_err(|err| map_auth_machine_error(err, context))?;
            let to_phase = map_phase(entry.state().lifecycle_phase);
            (input, from_phase, to_phase)
        };
        emit_audit(
            lease_key,
            Self::audit_action_for(&input),
            from_phase,
            to_phase,
        );
        Ok(())
    }

    fn restore_auth_lifecycle_snapshot(
        &self,
        captured: &AuthLeaseRestoreSnapshot,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        if captured.captured_by_type_id() != std::any::TypeId::of::<RuntimeAuthLeaseHandle>()
            || captured.captured_by_instance_id() != self.auth_lifecycle_restore_instance_id()
        {
            return Err(DslTransitionError::new(
                "AuthLeaseHandle::restore_auth_lifecycle_snapshot",
                "auth lifecycle restore snapshot was not captured from this RuntimeAuthLeaseHandle",
            ));
        }
        let lease_key = captured.lease_key();
        let snapshot = captured.snapshot();
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let from_phase = guard
            .authorities
            .get(lease_key)
            .map(|authority| map_phase(authority.state().lifecycle_phase))
            .unwrap_or(AuthLeasePhase::Released);
        let existing_oauth = guard
            .authorities
            .get(lease_key)
            .map(|authority| authority.state().clone());
        let has_oauth_membership = existing_oauth.as_ref().is_some_and(has_oauth_membership);

        if !snapshot.credential_present
            || snapshot.phase.is_none()
            || snapshot.phase == Some(AuthLeasePhase::Released)
        {
            let to_phase = if has_oauth_membership {
                let (to_phase, _) = apply_restore_input_to_registry(
                    &mut guard,
                    lease_key,
                    restore_input_from_lifecycle(
                        auth_dsl::AuthLifecyclePhase::ReauthRequired,
                        None,
                        None,
                        0,
                        false,
                        snapshot.generation,
                        None,
                    ),
                    "AuthLeaseHandle::restore_auth_lifecycle_snapshot",
                )?;
                to_phase
            } else {
                let (to_phase, _) = apply_restore_input_to_registry(
                    &mut guard,
                    lease_key,
                    restore_input_from_lifecycle(
                        auth_dsl::AuthLifecyclePhase::Released,
                        None,
                        None,
                        0,
                        false,
                        snapshot.generation,
                        None,
                    ),
                    "AuthLeaseHandle::restore_auth_lifecycle_snapshot",
                )?;
                to_phase
            };
            emit_audit(
                lease_key,
                "restore_auth_lifecycle_snapshot",
                from_phase,
                to_phase,
            );
            return Ok(None);
        }

        let Some(phase) = snapshot.phase else {
            return Ok(None);
        };
        let expires_at = snapshot.expires_at;
        let (to_phase, auth_transition) = apply_restore_input_to_registry(
            &mut guard,
            lease_key,
            restore_input_from_lifecycle(
                restore_phase(phase),
                expires_at,
                None,
                0,
                true,
                snapshot.generation,
                snapshot.credential_published_at_millis,
            ),
            "AuthLeaseHandle::restore_auth_lifecycle_snapshot",
        )?;
        emit_audit(
            lease_key,
            "restore_auth_lifecycle_snapshot",
            from_phase,
            to_phase,
        );
        Ok(Some(auth_transition))
    }

    fn auth_lifecycle_restore_instance_id(&self) -> usize {
        Arc::as_ptr(&self.machines) as usize
    }

    fn snapshot(&self, lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        let guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.authorities.get(lease_key) {
            Some(machine) => {
                let state = machine.state();
                let phase = (state.lifecycle_phase != auth_dsl::AuthLifecyclePhase::Released)
                    .then(|| map_phase(state.lifecycle_phase));
                AuthLeaseSnapshot {
                    phase,
                    expires_at: state.expires_at,
                    credential_present: state.credential_present,
                    generation: state.credential_generation,
                    credential_published_at_millis: state
                        .credential_present
                        .then_some(state.credential_published_at_millis)
                        .flatten(),
                }
            }
            None => AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                credential_present: false,
                generation: 0,
                credential_published_at_millis: None,
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::connection::{BindingId, RealmId};

    fn lease(realm: &str, binding: &str) -> LeaseKey {
        LeaseKey::new(
            RealmId::parse(realm).expect("valid realm"),
            BindingId::parse(binding).expect("valid binding"),
            None,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn auth_binding(realm: &str, binding: &str) -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse(realm).expect("valid realm"),
            binding: BindingId::parse(binding).expect("valid binding"),
            profile: None,
        }
    }

    #[test]
    fn acquire_and_snapshot_roundtrip() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "default_openai");
        h.acquire_lease(&key, 1_800_000_000).unwrap();
        let snap = h.snapshot(&key);
        assert_eq!(snap.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(snap.expires_at, Some(1_800_000_000));
    }

    #[test]
    fn lifecycle_transitions() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = lease("dev", "default_anthropic");

        h.acquire_lease(&k, 1_800_000_000).unwrap();
        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::Valid));

        h.mark_expiring(&k).unwrap();
        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::Expiring));

        h.begin_refresh(&k).unwrap();
        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::Refreshing));

        h.complete_refresh(&k, 1_800_000_900, 1_800_000_000)
            .unwrap();
        let snap = h.snapshot(&k);
        assert_eq!(snap.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(snap.expires_at, Some(1_800_000_900));
    }

    #[test]
    fn refresh_failed_permanent_routes_to_reauth() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = lease("dev", "default_google");

        h.acquire_lease(&k, 1_800_000_000).unwrap();
        h.begin_refresh(&k).unwrap();
        h.refresh_failed(&k, true).unwrap();

        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::ReauthRequired));
    }

    #[test]
    fn refresh_failed_transient_routes_to_expiring() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = lease("dev", "foo");

        h.acquire_lease(&k, 1_800_000_000).unwrap();
        h.begin_refresh(&k).unwrap();
        h.refresh_failed(&k, false).unwrap();

        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::Expiring));
    }

    #[test]
    fn transient_refresh_failure_preserves_credential_marker_generation() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = lease("dev", "retryable_refresh");

        h.acquire_lease(&k, 1_800_000_000).unwrap();
        let before = h.snapshot(&k);
        h.begin_refresh(&k).unwrap();
        h.refresh_failed(&k, false).unwrap();
        let after = h.snapshot(&k);

        assert_eq!(after.phase, Some(AuthLeasePhase::Expiring));
        assert_eq!(after.expires_at, before.expires_at);
        assert_eq!(after.generation, before.generation);
        assert_eq!(
            after.credential_published_at_millis,
            before.credential_published_at_millis
        );
    }

    #[test]
    fn snapshot_for_unknown_binding_is_none() {
        let h = RuntimeAuthLeaseHandle::new();
        let snap = h.snapshot(&lease("dev", "never_registered"));
        assert!(snap.phase.is_none());
        assert!(snap.expires_at.is_none());
    }

    #[test]
    fn mark_expiring_before_acquire_errors() {
        let h = RuntimeAuthLeaseHandle::new();
        let err = h.mark_expiring(&lease("dev", "ghost")).unwrap_err();
        assert_eq!(err.context, "AuthLeaseHandle::mark_expiring");
    }

    #[test]
    fn release_before_acquire_is_idempotent() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "ghost");

        h.release_lease(&key).unwrap();

        let snap = h.snapshot(&key);
        assert!(snap.phase.is_none());
        assert!(snap.expires_at.is_none());
    }

    #[test]
    fn release_does_not_remove_concurrent_reacquire() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "shared");
        h.acquire_lease(&key, 1_800_000_000).unwrap();

        let acquire_handle = h.clone();
        let acquire_key = key.clone();
        let hook_key = key.clone();
        let acquired_generation = Arc::new(Mutex::new(None));
        let hook_generation = Arc::clone(&acquired_generation);
        let _hook_guard =
            install_release_after_accept_hook_for_test(Arc::new(move |released_key| {
                if released_key != &hook_key {
                    return;
                }
                let generation = acquire_handle
                    .acquire_lease(&acquire_key, 1_800_000_000)
                    .unwrap()
                    .generation();
                *hook_generation.lock().unwrap() = Some(generation);
            }));

        h.release_lease(&key).unwrap();

        let acquired_generation = acquired_generation
            .lock()
            .unwrap()
            .expect("release hook should reacquire the lease");

        let snap = h.snapshot(&key);
        assert_eq!(
            snap.phase,
            Some(AuthLeasePhase::Valid),
            "accepted reacquire generation {acquired_generation} must remain visible after release completes; snapshot was {snap:?}"
        );
        assert_eq!(snap.expires_at, Some(1_800_000_000));
        assert_eq!(snap.generation, acquired_generation);
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct FailingReleaseObserver;

    #[cfg(not(target_arch = "wasm32"))]
    impl AuthLeaseReleaseObserver for FailingReleaseObserver {
        fn auth_lease_released(
            &self,
            _released: &ReleasedOAuthFlows,
        ) -> Result<(), DslTransitionError> {
            Err(DslTransitionError::new(
                "test_release_observer",
                "injected release observer failure",
            ))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn release_observer_failure_does_not_overwrite_concurrent_reacquire() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "shared_failure");
        h.acquire_lease(&key, 1_800_000_000).unwrap();

        let observer: Arc<dyn AuthLeaseReleaseObserver> = Arc::new(FailingReleaseObserver);
        h.add_release_observer(Arc::downgrade(&observer));

        let acquire_handle = h.clone();
        let acquire_key = key.clone();
        let hook_key = key.clone();
        let acquired_transition = Arc::new(Mutex::new(None));
        let hook_transition = Arc::clone(&acquired_transition);
        let _hook_guard =
            install_release_after_accept_hook_for_test(Arc::new(move |released_key| {
                if released_key != &hook_key {
                    return;
                }
                let transition = acquire_handle
                    .acquire_lease(&acquire_key, 1_900_000_000)
                    .unwrap();
                *hook_transition.lock().unwrap() = Some(transition);
            }));

        assert!(h.release_lease(&key).is_err());

        let acquired_transition = acquired_transition
            .lock()
            .unwrap()
            .clone()
            .expect("release hook should reacquire the lease");
        let snap = h.snapshot(&key);
        assert_eq!(snap.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(
            snap.expires_at,
            Some(1_900_000_000),
            "observer-failure rollback must not restore the stale released credential snapshot"
        );
        assert_eq!(snap.generation, acquired_transition.generation());
        assert_eq!(
            snap.credential_published_at_millis,
            acquired_transition.credential_published_at_millis()
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn release_observer_failure_does_not_resurrect_after_newer_release() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "shared_newer_release");
        h.acquire_lease(&key, 1_800_000_000).unwrap();

        let observer: Arc<dyn AuthLeaseReleaseObserver> = Arc::new(FailingReleaseObserver);
        h.add_release_observer(Arc::downgrade(&observer));

        let lifecycle_handle = h.clone();
        let lifecycle_key = key.clone();
        let hook_key = key.clone();
        let acquired_transition = Arc::new(Mutex::new(None));
        let hook_transition = Arc::clone(&acquired_transition);
        let _hook_guard =
            install_release_after_accept_hook_for_test(Arc::new(move |released_key| {
                if released_key != &hook_key {
                    return;
                }
                let transition = lifecycle_handle
                    .acquire_lease(&lifecycle_key, 1_900_000_000)
                    .unwrap();
                lifecycle_handle
                    .release_credential_lifecycle(&lifecycle_key)
                    .unwrap();
                *hook_transition.lock().unwrap() = Some(transition);
            }));

        assert!(h.release_lease(&key).is_err());

        let acquired_transition = acquired_transition
            .lock()
            .unwrap()
            .clone()
            .expect("release hook should reacquire before newer release");
        let snap = h.snapshot(&key);
        assert_eq!(
            snap.phase, None,
            "older observer-failure rollback must not resurrect stale credential state after a newer release"
        );
        assert_eq!(snap.expires_at, None);
        assert_eq!(snap.generation, acquired_transition.generation());
        assert_eq!(snap.credential_published_at_millis, None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_global_capacity_rejection_comes_from_generated_authority() {
        let h = RuntimeAuthLeaseHandle::new();
        let first = auth_binding("dev", "oauth_a");
        let second = auth_binding("dev", "oauth_b");

        h.apply_oauth_input(
            &first,
            auth_dsl::AuthMachineInput::AdmitOAuthBrowserFlow {
                flow_id: "first".to_string(),
                provider: "provider".to_string(),
                redirect_uri: "http://localhost/first".to_string(),
                expires_at_millis: 1_900_000_000,
                max_outstanding_flows: 1,
                observed_global_outstanding_flows: u64::MAX,
            },
            "test_admit_oauth_browser_flow",
            true,
        )
        .unwrap();

        let err = h
            .apply_oauth_input(
                &second,
                auth_dsl::AuthMachineInput::AdmitOAuthBrowserFlow {
                    flow_id: "second".to_string(),
                    provider: "provider".to_string(),
                    redirect_uri: "http://localhost/second".to_string(),
                    expires_at_millis: 1_900_000_000,
                    max_outstanding_flows: 1,
                    observed_global_outstanding_flows: 0,
                },
                "test_admit_oauth_browser_flow",
                true,
            )
            .unwrap_err();

        assert!(
            err.is_guard_rejected(),
            "expected generated guard rejection: {err:?}"
        );
        assert_eq!(err.context, "test_admit_oauth_browser_flow");
        assert!(
            err.reason.contains("AdmitOAuthBrowserFlow"),
            "guard rejection should come from the generated AuthMachine transition: {err:?}"
        );
        assert!(h.has_oauth_browser_flow_for_test(&first, "first"));
        assert!(!h.has_oauth_browser_flow_for_test(&second, "second"));
    }

    #[test]
    fn repeated_acquire_updates_existing_lease() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "default");

        h.acquire_lease(&key, 1_800_000_000).unwrap();
        h.acquire_lease(&key, 1_900_000_000).unwrap();

        let snap = h.snapshot(&key);
        assert_eq!(snap.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(snap.expires_at, Some(1_900_000_000));
    }

    #[test]
    fn restore_snapshot_preserves_publication_marker_without_lowering_generation() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "shared");

        h.acquire_lease(&key, 1_800_000_000).unwrap();
        let before = h.snapshot(&key);
        let before_restore = h.capture_auth_lifecycle_restore_snapshot(&key);
        assert_eq!(before.phase, Some(AuthLeasePhase::Valid));
        assert!(before.credential_present);
        assert!(before.credential_published_at_millis.is_some());

        h.acquire_lease(&key, 1_900_000_000).unwrap();
        let advanced = h.snapshot(&key);
        assert!(advanced.generation > before.generation);

        h.restore_auth_lifecycle_snapshot(&before_restore).unwrap();

        let restored = h.snapshot(&key);
        assert_eq!(restored.phase, before.phase);
        assert_eq!(restored.expires_at, before.expires_at);
        assert_eq!(restored.credential_present, before.credential_present);
        assert_eq!(
            restored.credential_published_at_millis,
            before.credential_published_at_millis
        );
        assert_eq!(restored.generation, advanced.generation);
    }

    #[test]
    fn restore_empty_zero_generation_snapshot_releases_through_generated_authority() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "shared");
        let empty = h.capture_auth_lifecycle_restore_snapshot(&key);
        h.acquire_lease(&key, 1_800_000_000).unwrap();
        let acquired_generation = h.snapshot(&key).generation;
        assert!(acquired_generation > empty.snapshot().generation);

        h.restore_auth_lifecycle_snapshot(&empty).unwrap();

        let restored = h.snapshot(&key);
        assert_eq!(restored.phase, None);
        assert_eq!(restored.expires_at, None);
        assert!(!restored.credential_present);
        assert_eq!(restored.generation, acquired_generation);
        assert_eq!(restored.credential_published_at_millis, None);
    }

    #[test]
    fn restore_snapshot_rejects_capture_from_different_runtime_handle() {
        let first = RuntimeAuthLeaseHandle::new();
        let second = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "shared");
        first.acquire_lease(&key, 1_800_000_000).unwrap();
        let captured = first.capture_auth_lifecycle_restore_snapshot(&key);

        let err = second
            .restore_auth_lifecycle_snapshot(&captured)
            .unwrap_err();

        assert_eq!(
            err.context,
            "AuthLeaseHandle::restore_auth_lifecycle_snapshot"
        );
        assert!(
            err.reason.contains("this RuntimeAuthLeaseHandle"),
            "restore must reject snapshots captured from a different runtime handle: {err:?}"
        );
        assert_eq!(second.snapshot(&key).phase, None);
    }

    #[test]
    fn per_binding_isolation() {
        let h = RuntimeAuthLeaseHandle::new();
        let openai = lease("dev", "openai");
        let anthropic = lease("dev", "anthropic");
        h.acquire_lease(&openai, 1_800_000_000).unwrap();
        h.acquire_lease(&anthropic, 1_900_000_000).unwrap();
        h.mark_expiring(&openai).unwrap();

        assert_eq!(h.snapshot(&openai).phase, Some(AuthLeasePhase::Expiring));
        assert_eq!(h.snapshot(&anthropic).phase, Some(AuthLeasePhase::Valid));
        assert_eq!(h.snapshot(&anthropic).expires_at, Some(1_900_000_000));
    }
}
