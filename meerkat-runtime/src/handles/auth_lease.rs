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
use meerkat_core::RefreshFailureObservation;
use meerkat_core::auth::TokenKey;
use meerkat_core::generated::auth_lease_durable_lifecycle_marker::AuthLeaseDurableRestorePublication;
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

fn restore_phase_to_dsl(phase: AuthLeasePhase) -> auth_dsl::AuthLifecyclePhase {
    match phase {
        AuthLeasePhase::Valid => auth_dsl::AuthLifecyclePhase::Valid,
        AuthLeasePhase::Expiring => auth_dsl::AuthLifecyclePhase::Expiring,
        AuthLeasePhase::Expired => auth_dsl::AuthLifecyclePhase::Expired,
        AuthLeasePhase::Refreshing => auth_dsl::AuthLifecyclePhase::Refreshing,
        AuthLeasePhase::ReauthRequired => auth_dsl::AuthLifecyclePhase::ReauthRequired,
        AuthLeasePhase::Released => auth_dsl::AuthLifecyclePhase::Released,
    }
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

fn restore_credential_lifecycle_snapshot_input(
    lifecycle_phase: Option<auth_dsl::AuthLifecyclePhase>,
    expires_at: Option<u64>,
    last_refresh: Option<u64>,
    refresh_attempt: u64,
    credential_present: bool,
    credential_generation: u64,
    credential_published_at_millis: Option<u64>,
    restored_oauth_membership_observed: bool,
) -> auth_dsl::AuthMachineInput {
    auth_dsl::AuthMachineInput::RestoreCredentialLifecycleSnapshot {
        lifecycle_phase,
        expires_at,
        last_refresh,
        refresh_attempt,
        credential_present,
        credential_generation,
        credential_published_at_millis,
        restored_oauth_membership_observed,
    }
}

fn append_restore_oauth_inputs_from_state(
    inputs: &mut Vec<auth_dsl::AuthMachineInput>,
    poll_inputs: &mut Vec<auth_dsl::AuthMachineInput>,
    state: &auth_dsl::AuthMachineState,
) {
    for flow_id in &state.oauth_browser_flow_ids {
        inputs.push(auth_dsl::AuthMachineInput::RestoreOAuthBrowserFlow {
            flow_id: flow_id.clone(),
            provider: state.oauth_browser_flow_providers.get(flow_id).cloned(),
            redirect_uri: state.oauth_browser_flow_redirect_uris.get(flow_id).cloned(),
            expires_at_millis: state
                .oauth_browser_flow_expires_at_millis
                .get(flow_id)
                .copied(),
        });
    }
    for flow_id in &state.oauth_device_flow_ids {
        inputs.push(auth_dsl::AuthMachineInput::RestoreOAuthDeviceFlow {
            flow_id: flow_id.clone(),
            provider: state.oauth_device_flow_providers.get(flow_id).cloned(),
            expires_at_millis: state
                .oauth_device_flow_expires_at_millis
                .get(flow_id)
                .copied(),
        });
    }
    for poll_id in &state.oauth_device_poll_ids {
        poll_inputs.push(auth_dsl::AuthMachineInput::RestoreOAuthDevicePoll {
            flow_id: poll_id.clone(),
        });
    }
}

fn restore_oauth_inputs_from_states(
    states: &[&auth_dsl::AuthMachineState],
) -> Vec<auth_dsl::AuthMachineInput> {
    let mut inputs = Vec::new();
    let mut poll_inputs = Vec::new();
    for state in states {
        append_restore_oauth_inputs_from_state(&mut inputs, &mut poll_inputs, state);
    }
    inputs.extend(poll_inputs);
    inputs
}

fn restore_oauth_membership_observed(states: &[&auth_dsl::AuthMachineState]) -> bool {
    states
        .iter()
        .any(|state| state.oauth_outstanding_flow_count > 0)
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
        auth_dsl::AuthMachineTransitionError::RecoveredStateInvariantRejected { .. } => {
            DslTransitionError::recovered_state_invariant_rejected(context, reason)
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

fn restore_authority_from_registry(
    registry: &AuthLeaseRegistry,
    lease_key: &LeaseKey,
    context: &'static str,
) -> Result<auth_dsl::AuthMachineAuthority, DslTransitionError> {
    match registry.authorities.get(lease_key) {
        Some(authority) => {
            auth_dsl::AuthMachineAuthority::recover_from_state(authority.state().clone())
                .map_err(|err| map_auth_machine_error(err, context))
        }
        None => Ok(auth_dsl::AuthMachineAuthority::new()),
    }
}

fn apply_restore_inputs_to_registry(
    registry: &mut AuthLeaseRegistry,
    lease_key: &LeaseKey,
    lifecycle_input: auth_dsl::AuthMachineInput,
    oauth_inputs: Vec<auth_dsl::AuthMachineInput>,
    context: &'static str,
) -> Result<(AuthLeasePhase, AuthLeaseTransition), DslTransitionError> {
    let mut authority = restore_authority_from_registry(registry, lease_key, context)?;
    let restored = apply_restore_input(&mut authority, lease_key, lifecycle_input, context)?;
    for input in oauth_inputs {
        apply_restore_input(&mut authority, lease_key, input, context)?;
    }
    registry.authorities.insert(lease_key.clone(), authority);
    Ok(restored)
}

fn restore_state_to_registry(
    registry: &mut AuthLeaseRegistry,
    lease_key: &LeaseKey,
    state: &auth_dsl::AuthMachineState,
    context: &'static str,
) -> Result<(AuthLeasePhase, AuthLeaseTransition), DslTransitionError> {
    restore_state_with_oauth_sources_to_registry(registry, lease_key, state, &[state], context)
}

fn restore_state_with_oauth_sources_to_registry(
    registry: &mut AuthLeaseRegistry,
    lease_key: &LeaseKey,
    lifecycle_state: &auth_dsl::AuthMachineState,
    oauth_sources: &[&auth_dsl::AuthMachineState],
    context: &'static str,
) -> Result<(AuthLeasePhase, AuthLeaseTransition), DslTransitionError> {
    let oauth_inputs = restore_oauth_inputs_from_states(oauth_sources);
    apply_restore_inputs_to_registry(
        registry,
        lease_key,
        restore_credential_lifecycle_snapshot_input(
            Some(lifecycle_state.lifecycle_phase),
            lifecycle_state.expires_at,
            lifecycle_state.last_refresh,
            lifecycle_state.refresh_attempt,
            lifecycle_state.credential_present,
            lifecycle_state.credential_generation,
            lifecycle_state.credential_published_at_millis,
            restore_oauth_membership_observed(oauth_sources),
        ),
        oauth_inputs,
        context,
    )
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
    let oauth_sources = [oauth];
    let oauth_inputs = restore_oauth_inputs_from_states(&oauth_sources);
    apply_restore_inputs_to_registry(
        registry,
        lease_key,
        restore_credential_lifecycle_snapshot_input(
            Some(lifecycle_phase),
            expires_at,
            last_refresh,
            refresh_attempt,
            credential_present,
            credential_generation,
            credential_published_at_millis,
            restore_oauth_membership_observed(&oauth_sources),
        ),
        oauth_inputs,
        context,
    )
}

fn auth_lease_transition_from_generated_publication(
    lease_key: &LeaseKey,
    authority: &auth_dsl::AuthMachineAuthority,
    transition: &auth_dsl::AuthMachineTransition,
    context: &'static str,
) -> Result<AuthLeaseTransition, DslTransitionError> {
    match maybe_auth_lease_transition_from_generated_publication(
        lease_key, authority, transition, context,
    )? {
        Some(transition) => Ok(transition),
        None => Err(DslTransitionError::new(
            context,
            "AuthMachine transition emitted no lifecycle publication obligation",
        )),
    }
}

fn maybe_auth_lease_transition_from_generated_publication(
    lease_key: &LeaseKey,
    authority: &auth_dsl::AuthMachineAuthority,
    transition: &auth_dsl::AuthMachineTransition,
    context: &'static str,
) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
    let mut obligations =
        crate::protocol_auth_lease_lifecycle_publication::extract_obligations(transition);
    if obligations.is_empty() {
        return Ok(None);
    }
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
        .map(Some)
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

fn registry_phase_or_released(
    registry: &AuthLeaseRegistry,
    lease_key: &LeaseKey,
) -> AuthLeasePhase {
    registry
        .authorities
        .get(lease_key)
        .map(|current| map_phase(current.state().lifecycle_phase))
        .unwrap_or(AuthLeasePhase::Released)
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
            let transition = auth_dsl::AuthMachineMutator::apply(
                &mut authority,
                auth_dsl::AuthMachineInput::MarkReauthRequired,
            )
            .map_err(|err| map_auth_machine_error(err, context))?;
            auth_lease_transition_from_generated_publication(
                &lease_key,
                &authority,
                &transition,
                context,
            )?;
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
            let transition = auth_dsl::AuthMachineMutator::apply(entry, input)
                .map_err(|err| map_auth_machine_error(err, context))?;
            maybe_auth_lease_transition_from_generated_publication(
                &lease_key,
                entry,
                &transition,
                context,
            )?;
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
            let transition = auth_dsl::AuthMachineMutator::apply(entry, input)
                .map_err(|err| map_auth_machine_error(err, context))?;
            maybe_auth_lease_transition_from_generated_publication(
                &lease_key,
                entry,
                &transition,
                context,
            )?;
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
        let restore_result = if current_generation > previous_generation_value {
            match previous_state {
                Some(previous_state) => {
                    if let Some(current_state) = guard
                        .authorities
                        .get(lease_key)
                        .map(|current| current.state().clone())
                    {
                        restore_state_with_oauth_sources_to_registry(
                            &mut guard,
                            lease_key,
                            &current_state,
                            &[&current_state, &previous_state],
                            CONTEXT,
                        )
                        .map(|(phase, _)| phase)
                    } else {
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
                    }
                }
                None => Ok(registry_phase_or_released(&guard, lease_key)),
            }
        } else if let Some(restored_state) = previous_state {
            match guard
                .authorities
                .get(lease_key)
                .map(|current| current.state().clone())
            {
                Some(current_state) if current_state.credential_present => {
                    restore_state_with_oauth_sources_to_registry(
                        &mut guard,
                        lease_key,
                        &current_state,
                        &[&current_state, &restored_state],
                        CONTEXT,
                    )
                    .map(|(phase, _)| phase)
                }
                Some(current_state) => restore_state_with_oauth_sources_to_registry(
                    &mut guard,
                    lease_key,
                    &restored_state,
                    &[&restored_state, &current_state],
                    CONTEXT,
                )
                .map(|(phase, _)| phase),
                None => restore_state_to_registry(&mut guard, lease_key, &restored_state, CONTEXT)
                    .map(|(phase, _)| phase),
            }
        } else {
            Ok(registry_phase_or_released(&guard, lease_key))
        };
        let to_phase = restore_result.unwrap_or_else(|err| {
            tracing::error!(
                %lease_key,
                error = %err,
                "generated AuthMachine rollback after release observer failure was rejected"
            );
            registry_phase_or_released(&guard, lease_key)
        });
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
            auth_dsl::AuthMachineInput::ObserveCredentialFreshness { .. } => {
                "observe_credential_freshness"
            }
            auth_dsl::AuthMachineInput::BeginRefresh => "begin_refresh",
            auth_dsl::AuthMachineInput::CompleteRefresh { .. } => "complete_refresh",
            auth_dsl::AuthMachineInput::RefreshFailed { .. } => "refresh_failed",
            auth_dsl::AuthMachineInput::MarkReauthRequired => "mark_reauth_required",
            auth_dsl::AuthMachineInput::ClearCredentialLifecycle => "clear_credential_lifecycle",
            auth_dsl::AuthMachineInput::ReleaseCredentialLifecycle => {
                "release_credential_lifecycle"
            }
            auth_dsl::AuthMachineInput::Release => "release_lease",
            auth_dsl::AuthMachineInput::RestoreAuthoritySnapshot { .. } => {
                "restore_authority_snapshot"
            }
            auth_dsl::AuthMachineInput::RestoreCredentialLifecycleSnapshot { .. } => {
                "restore_credential_lifecycle_snapshot"
            }
            auth_dsl::AuthMachineInput::RestoreOAuthBrowserFlow { .. } => {
                "restore_oauth_browser_flow"
            }
            auth_dsl::AuthMachineInput::RestoreOAuthDeviceFlow { .. } => {
                "restore_oauth_device_flow"
            }
            auth_dsl::AuthMachineInput::RestoreOAuthDevicePoll { .. } => {
                "restore_oauth_device_poll"
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
        auth_dsl::AuthLifecyclePhase::Expired => AuthLeasePhase::Expired,
        auth_dsl::AuthLifecyclePhase::Refreshing => AuthLeasePhase::Refreshing,
        auth_dsl::AuthLifecyclePhase::ReauthRequired => AuthLeasePhase::ReauthRequired,
        auth_dsl::AuthLifecyclePhase::Released => AuthLeasePhase::Released,
    }
}

fn restore_phase(phase: AuthLeasePhase) -> auth_dsl::AuthLifecyclePhase {
    match phase {
        AuthLeasePhase::Valid => auth_dsl::AuthLifecyclePhase::Valid,
        AuthLeasePhase::Expiring => auth_dsl::AuthLifecyclePhase::Expiring,
        AuthLeasePhase::Expired => auth_dsl::AuthLifecyclePhase::Expired,
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

    fn observe_credential_freshness(
        &self,
        lease_key: &LeaseKey,
        now: u64,
        refresh_window_secs: u64,
    ) -> Result<(), DslTransitionError> {
        if !self
            .machines
            .lock()
            .unwrap()
            .authorities
            .contains_key(lease_key)
        {
            return Ok(());
        }
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::ObserveCredentialFreshness {
                now_ts: now,
                refresh_window_secs,
            },
            "AuthLeaseHandle::observe_credential_freshness",
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
        observation: RefreshFailureObservation,
    ) -> Result<(), DslTransitionError> {
        let input = auth_dsl::AuthMachineInput::RefreshFailed {
            http_status: observation.http_status,
            oauth_error_code: observation.oauth_error_code,
            local_credential_unusable: observation.local_credential_unusable,
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
            let transition =
                auth_dsl::AuthMachineMutator::apply(entry, auth_dsl::AuthMachineInput::Release)
                    .map_err(|err| map_auth_machine_error(err, context))?;
            auth_lease_transition_from_generated_publication(
                lease_key,
                entry,
                &transition,
                context,
            )?;
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
            let input = auth_dsl::AuthMachineInput::ReleaseCredentialLifecycle;
            let from_phase = map_phase(entry.state().lifecycle_phase);
            let transition = auth_dsl::AuthMachineMutator::apply(entry, input.clone())
                .map_err(|err| map_auth_machine_error(err, context))?;
            auth_lease_transition_from_generated_publication(
                lease_key,
                entry,
                &transition,
                context,
            )?;
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
        let (to_phase, auth_transition) = apply_restore_input_to_registry(
            &mut guard,
            lease_key,
            restore_credential_lifecycle_snapshot_input(
                snapshot.phase.map(restore_phase),
                snapshot.expires_at,
                None,
                0,
                snapshot.credential_present,
                snapshot.generation,
                snapshot.credential_published_at_millis,
                false,
            ),
            "AuthLeaseHandle::restore_auth_lifecycle_snapshot",
        )?;
        emit_audit(
            lease_key,
            "restore_auth_lifecycle_snapshot",
            from_phase,
            to_phase,
        );
        if snapshot.credential_present
            && snapshot.phase.is_some()
            && snapshot.phase != Some(AuthLeasePhase::Released)
        {
            Ok(Some(auth_transition))
        } else {
            Ok(None)
        }
    }

    fn auth_lifecycle_restore_instance_id(&self) -> usize {
        Arc::as_ptr(&self.machines) as usize
    }

    fn restore_published_credential_lifecycle(
        &self,
        lease_key: &LeaseKey,
        publication: &AuthLeaseDurableRestorePublication,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let context = "AuthLeaseHandle::restore_published_credential_lifecycle";
        let lease_token_key = TokenKey::new_with_profile(
            lease_key.realm.clone(),
            lease_key.binding.clone(),
            lease_key.profile.clone(),
        );
        if publication.token_key() != &lease_token_key {
            return Err(DslTransitionError::new(
                context,
                "durable auth lifecycle marker identity does not match restore lease key",
            ));
        }
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let from_phase = guard
            .authorities
            .get(lease_key)
            .map(|authority| map_phase(authority.state().lifecycle_phase))
            .unwrap_or(AuthLeasePhase::Released);
        let (to_phase, auth_transition) = apply_restore_input_to_registry(
            &mut guard,
            lease_key,
            restore_input_from_lifecycle(
                restore_phase_to_dsl(publication.phase()),
                (publication.expires_at() != u64::MAX).then_some(publication.expires_at()),
                None,
                0,
                publication.phase() != AuthLeasePhase::Released,
                publication.generation(),
                Some(publication.credential_published_at_millis()),
            ),
            context,
        )?;
        emit_audit(
            lease_key,
            "restore_published_credential_lifecycle",
            from_phase,
            to_phase,
        );
        Ok(auth_transition)
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

    fn empty_auth_state() -> auth_dsl::AuthMachineState {
        auth_dsl::AuthMachineAuthority::new().state().clone()
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
    fn observe_credential_freshness_marks_expired_through_authmachine() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = lease("dev", "expired_openai");

        h.acquire_lease(&k, 1_800_000_000).unwrap();
        h.observe_credential_freshness(&k, 1_800_000_001, 60)
            .unwrap();

        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::Expired));
        h.begin_refresh(&k).unwrap();
        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::Refreshing));
    }

    #[test]
    fn refresh_failed_permanent_routes_to_reauth() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = lease("dev", "default_google");

        h.acquire_lease(&k, 1_800_000_000).unwrap();
        h.begin_refresh(&k).unwrap();
        h.refresh_failed(&k, RefreshFailureObservation::local_credential_unusable())
            .unwrap();

        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::ReauthRequired));
    }

    #[test]
    fn refresh_failed_transient_routes_to_expiring() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = lease("dev", "foo");

        h.acquire_lease(&k, 1_800_000_000).unwrap();
        h.begin_refresh(&k).unwrap();
        h.refresh_failed(&k, RefreshFailureObservation::transient())
            .unwrap();

        assert_eq!(h.snapshot(&k).phase, Some(AuthLeasePhase::Expiring));
    }

    #[test]
    fn transient_refresh_failure_preserves_credential_marker_generation() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = lease("dev", "retryable_refresh");

        h.acquire_lease(&k, 1_800_000_000).unwrap();
        let before = h.snapshot(&k);
        h.begin_refresh(&k).unwrap();
        h.refresh_failed(&k, RefreshFailureObservation::transient())
            .unwrap();
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
    fn restore_oauth_missing_payload_rejected_by_generated_authority() {
        let mut registry = AuthLeaseRegistry::default();
        let key = lease("dev", "restore_missing_provider");
        let mut state = empty_auth_state();
        state.lifecycle_phase = auth_dsl::AuthLifecyclePhase::ReauthRequired;
        state
            .oauth_browser_flow_ids
            .insert("browser-flow".to_string());
        state.oauth_browser_flow_redirect_uris.insert(
            "browser-flow".to_string(),
            "http://localhost/callback".to_string(),
        );
        state
            .oauth_browser_flow_expires_at_millis
            .insert("browser-flow".to_string(), 1_900_000_000);
        state.oauth_outstanding_flow_count = 1;

        let err = restore_state_to_registry(
            &mut registry,
            &key,
            &state,
            "test_restore_oauth_missing_payload",
        )
        .unwrap_err();

        assert!(
            err.is_guard_rejected(),
            "missing restored OAuth payload must be rejected by generated guards: {err:?}"
        );
        assert!(
            err.reason.contains("RestoreOAuthBrowserFlow"),
            "rejection should name the generated restore input: {err:?}"
        );
        assert!(!registry.authorities.contains_key(&key));
    }

    #[test]
    fn restore_oauth_orphan_device_poll_rejected_by_generated_authority() {
        let mut registry = AuthLeaseRegistry::default();
        let key = lease("dev", "restore_orphan_poll");
        let mut state = empty_auth_state();
        state.lifecycle_phase = auth_dsl::AuthLifecyclePhase::ReauthRequired;
        state
            .oauth_device_poll_ids
            .insert("orphan-device".to_string());

        let err = restore_state_to_registry(
            &mut registry,
            &key,
            &state,
            "test_restore_oauth_orphan_device_poll",
        )
        .unwrap_err();

        assert!(
            err.is_guard_rejected(),
            "orphan restored OAuth poll must be rejected by generated guards: {err:?}"
        );
        assert!(
            err.reason.contains("RestoreOAuthDevicePoll"),
            "rejection should name the generated restore input: {err:?}"
        );
        assert!(!registry.authorities.contains_key(&key));
    }

    #[test]
    fn restore_released_oauth_membership_reauths_through_generated_authority() {
        let mut registry = AuthLeaseRegistry::default();
        let key = lease("dev", "restore_released_oauth");
        let mut state = empty_auth_state();
        state.lifecycle_phase = auth_dsl::AuthLifecyclePhase::Released;
        state
            .oauth_browser_flow_ids
            .insert("browser-flow".to_string());
        state
            .oauth_browser_flow_providers
            .insert("browser-flow".to_string(), "provider".to_string());
        state.oauth_browser_flow_redirect_uris.insert(
            "browser-flow".to_string(),
            "http://localhost/callback".to_string(),
        );
        state
            .oauth_browser_flow_expires_at_millis
            .insert("browser-flow".to_string(), 1_900_000_000);
        state.oauth_outstanding_flow_count = 1;

        let (phase, transition) = restore_state_to_registry(
            &mut registry,
            &key,
            &state,
            "test_restore_released_oauth_membership",
        )
        .unwrap();

        assert_eq!(phase, AuthLeasePhase::ReauthRequired);
        assert_eq!(transition.phase(), AuthLeasePhase::ReauthRequired);
        let restored = registry.authorities.get(&key).unwrap().state();
        assert_eq!(
            restored.lifecycle_phase,
            auth_dsl::AuthLifecyclePhase::ReauthRequired
        );
        assert!(restored.oauth_browser_flow_ids.contains("browser-flow"));
        assert_eq!(restored.oauth_outstanding_flow_count, 1);
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

    #[tokio::test]
    async fn restore_published_credential_lifecycle_uses_generated_authority() {
        struct SingleTokenStore {
            key: meerkat_core::auth::TokenKey,
            tokens: meerkat_core::auth::PersistedTokens,
        }

        #[async_trait::async_trait]
        impl meerkat_core::auth::TokenStore for SingleTokenStore {
            async fn load(
                &self,
                key: &meerkat_core::auth::TokenKey,
            ) -> Result<
                Option<meerkat_core::auth::PersistedTokens>,
                meerkat_core::auth::TokenStoreError,
            > {
                Ok((key == &self.key).then(|| self.tokens.clone()))
            }

            async fn save(
                &self,
                _key: &meerkat_core::auth::TokenKey,
                _tokens: &meerkat_core::auth::PersistedTokens,
            ) -> Result<(), meerkat_core::auth::TokenStoreError> {
                Ok(())
            }

            async fn clear(
                &self,
                _key: &meerkat_core::auth::TokenKey,
            ) -> Result<(), meerkat_core::auth::TokenStoreError> {
                Ok(())
            }

            async fn list(
                &self,
            ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
            {
                Ok(vec![self.key.clone()])
            }

            fn backend_name(&self) -> &'static str {
                "single-token-test"
            }
        }

        let source = Arc::new(RuntimeAuthLeaseHandle::new());
        let restored = Arc::new(RuntimeAuthLeaseHandle::new());
        let generated_restored =
            crate::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
                Arc::clone(&restored),
            )
            .expect("test AuthLeaseHandle must be generated-authority certified");
        let key = lease("dev", "shared");
        let transition = source.acquire_lease(&key, 1_800_000_000).unwrap();
        let published_at = transition
            .credential_published_at_millis()
            .expect("acquire transition carries publication time");
        let key_for_tokens = meerkat_core::auth::TokenKey::new_with_profile(
            key.realm.clone(),
            key.binding.clone(),
            key.profile.clone(),
        );
        let tokens = meerkat_core::auth::PersistedTokens {
            auth_mode: meerkat_core::auth::PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("access-token".into()),
            refresh_token: Some("refresh-token".into()),
            id_token: None,
            expires_at: Some(
                chrono::DateTime::from_timestamp(transition.expires_at() as i64, 0)
                    .expect("fixture expiry is representable"),
            ),
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        };
        let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &key_for_tokens,
            &tokens,
            &transition,
        )
        .expect("generated transition marks durable publication");

        let auth_binding = AuthBindingRef {
            realm: key.realm.clone(),
            binding: key.binding.clone(),
            profile: key.profile.clone(),
        };
        let store = SingleTokenStore {
            key: key_for_tokens,
            tokens: marked,
        };
        meerkat_core::rehydrate_marked_tokens_for_status(
            &store,
            &generated_restored,
            &auth_binding,
            meerkat_core::auth::PersistedAuthMode::ChatgptOauth,
            chrono::Utc::now(),
        )
        .await
        .expect("generated marker restores through AuthMachine")
        .expect("marker is present");

        let snapshot = restored.snapshot(&key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(snapshot.expires_at, Some(transition.expires_at()));
        assert_eq!(snapshot.generation, transition.generation());
        assert_eq!(snapshot.credential_published_at_millis, Some(published_at));
        assert!(snapshot.credential_present);
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
