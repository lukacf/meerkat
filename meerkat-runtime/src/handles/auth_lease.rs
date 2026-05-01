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
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::ConnectionRef;
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition, DslTransitionError,
    LeaseKey,
};

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
pub(crate) trait AuthLeaseReleaseObserver: Send + Sync {
    fn auth_lease_released(&self, released: &ReleasedOAuthFlows) -> Result<(), DslTransitionError>;
}

#[cfg(test)]
type ReleaseAfterAcceptHook = Arc<dyn Fn(&LeaseKey) + Send + Sync>;

#[cfg(test)]
static RELEASE_AFTER_ACCEPT_HOOK: std::sync::OnceLock<Mutex<Option<ReleaseAfterAcceptHook>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) fn set_release_after_accept_hook_for_test(hook: Option<ReleaseAfterAcceptHook>) {
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
    // Projection version for AuthLeaseHandle::snapshot consumers. This is not
    // lifecycle state; it is retained after release so authorizer-side token
    // material can detect a later reacquire even when the expiry is identical.
    generations: HashMap<LeaseKey, u64>,
    credential_published_at_millis: HashMap<LeaseKey, u64>,
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
    fn notify_release_observers(
        &self,
        released: &ReleasedOAuthFlows,
    ) -> Result<(), DslTransitionError> {
        let observers = {
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
        };
        for observer in observers {
            observer.auth_lease_released(released)?;
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn oauth_flow_bootstrap_state() -> auth_dsl::AuthMachineState {
        auth_dsl::AuthMachineState {
            lifecycle_phase: auth_dsl::AuthLifecyclePhase::ReauthRequired,
            ..Default::default()
        }
    }

    fn apply(
        &self,
        lease_key: &LeaseKey,
        input: auth_dsl::AuthMachineInput,
        context: &'static str,
        create_if_missing: bool,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let action = Self::audit_action_for(&input);
        let remove_after_accept = matches!(&input, auth_dsl::AuthMachineInput::Release);
        let publishes_credential = matches!(
            &input,
            auth_dsl::AuthMachineInput::Acquire { .. }
                | auth_dsl::AuthMachineInput::CompleteRefresh { .. }
        );
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !create_if_missing
            && guard.generations.get(lease_key).copied().unwrap_or(0) == 0
            && !remove_after_accept
        {
            return Err(DslTransitionError::new(
                context,
                format!("no auth lease registered for lease_key `{lease_key}`"),
            ));
        }
        let (from_phase, to_phase) = {
            let entry = if create_if_missing {
                guard
                    .authorities
                    .entry(lease_key.clone())
                    .or_insert_with(|| {
                        auth_dsl::AuthMachineAuthority::from_state(
                            auth_dsl::AuthMachineState::default(),
                        )
                    })
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
            let from_phase = map_phase(entry.state.lifecycle_phase);
            auth_dsl::AuthMachineMutator::apply(entry, input)
                .map_err(|err| DslTransitionError::new(context, format!("{err}")))?;
            let to_phase = map_phase(entry.state.lifecycle_phase);
            (from_phase, to_phase)
        };
        let generation = guard.generations.entry(lease_key.clone()).or_insert(0);
        if publishes_credential {
            *generation = generation.saturating_add(1);
        }
        let accepted_generation = *generation;
        let credential_published_at_millis = if publishes_credential {
            let published_at = current_time_millis();
            guard
                .credential_published_at_millis
                .insert(lease_key.clone(), published_at);
            Some(published_at)
        } else {
            guard.credential_published_at_millis.get(lease_key).copied()
        };
        if remove_after_accept {
            guard.authorities.remove(lease_key);
            guard.credential_published_at_millis.remove(lease_key);
        }
        emit_audit(lease_key, action, from_phase, to_phase);
        Ok(AuthLeaseTransition::new(
            accepted_generation,
            credential_published_at_millis,
        ))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn apply_oauth_input(
        &self,
        target: &ConnectionRef,
        input: auth_dsl::AuthMachineInput,
        context: &'static str,
        create_if_missing: bool,
    ) -> Result<(), DslTransitionError> {
        let lease_key = LeaseKey::from_connection_ref(target);
        let action = Self::audit_action_for(&input);
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((transition, max_outstanding_flows)) = match &input {
            auth_dsl::AuthMachineInput::AdmitOAuthBrowserFlow {
                max_outstanding_flows,
                ..
            } => Some(("AdmitOAuthBrowserFlow", *max_outstanding_flows)),
            auth_dsl::AuthMachineInput::AdmitOAuthDeviceFlow {
                max_outstanding_flows,
                ..
            } => Some(("AdmitOAuthDeviceFlow", *max_outstanding_flows)),
            _ => None,
        } {
            let outstanding = guard
                .authorities
                .values()
                .map(|authority| authority.state.oauth_outstanding_flow_count)
                .fold(0u64, u64::saturating_add);
            if outstanding >= max_outstanding_flows {
                return Err(DslTransitionError::guard_rejected(
                    context,
                    format!(
                        "transition {transition} guard oauth_global_capacity_available failed: {outstanding} outstanding OAuth flows >= {max_outstanding_flows} max"
                    ),
                ));
            }
        }
        let (from_phase, to_phase) = {
            let entry = if create_if_missing {
                guard
                    .authorities
                    .entry(lease_key.clone())
                    .or_insert_with(|| {
                        auth_dsl::AuthMachineAuthority::from_state(
                            Self::oauth_flow_bootstrap_state(),
                        )
                    })
            } else {
                match guard.authorities.get_mut(&lease_key) {
                    Some(m) => m,
                    None => {
                        return Err(DslTransitionError::new(
                            context,
                            format!("no auth machine registered for lease_key `{lease_key}`"),
                        ));
                    }
                }
            };
            let from_phase = map_phase(entry.state.lifecycle_phase);
            auth_dsl::AuthMachineMutator::apply(entry, input)
                .map_err(|err| DslTransitionError::new(context, format!("{err}")))?;
            let to_phase = map_phase(entry.state.lifecycle_phase);
            (from_phase, to_phase)
        };
        emit_audit(&lease_key, action, from_phase, to_phase);
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn has_oauth_browser_flow(&self, target: &ConnectionRef, flow_id: &str) -> bool {
        let lease_key = LeaseKey::from_connection_ref(target);
        self.machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .authorities
            .get(&lease_key)
            .is_some_and(|authority| authority.state.oauth_browser_flow_ids.contains(flow_id))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn has_oauth_device_flow(&self, target: &ConnectionRef, flow_id: &str) -> bool {
        let lease_key = LeaseKey::from_connection_ref(target);
        self.machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .authorities
            .get(&lease_key)
            .is_some_and(|authority| authority.state.oauth_device_flow_ids.contains(flow_id))
    }

    #[cfg(test)]
    pub(crate) fn has_oauth_browser_flow_for_test(
        &self,
        target: &ConnectionRef,
        flow_id: &str,
    ) -> bool {
        self.has_oauth_browser_flow(target, flow_id)
    }

    #[cfg(test)]
    pub(crate) fn has_oauth_device_flow_for_test(
        &self,
        target: &ConnectionRef,
        flow_id: &str,
    ) -> bool {
        self.has_oauth_device_flow(target, flow_id)
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
            auth_dsl::AuthMachineInput::Acquire { expires_at_ts },
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
        let released;
        let (from_phase, to_phase) = {
            let mut guard = self
                .machines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry =
                guard
                    .authorities
                    .entry(lease_key.clone())
                    .or_insert_with(|| {
                        auth_dsl::AuthMachineAuthority::from_state(
                            auth_dsl::AuthMachineState::default(),
                        )
                    });
            #[cfg(not(target_arch = "wasm32"))]
            {
                released = ReleasedOAuthFlows {
                    lease_key: lease_key.clone(),
                    browser_flow_ids: entry.state.oauth_browser_flow_ids.iter().cloned().collect(),
                    device_flow_ids: entry.state.oauth_device_flow_ids.iter().cloned().collect(),
                };
            }
            let from_phase = map_phase(entry.state.lifecycle_phase);
            auth_dsl::AuthMachineMutator::apply(entry, auth_dsl::AuthMachineInput::Release)
                .map_err(|err| DslTransitionError::new(context, format!("{err}")))?;
            let to_phase = map_phase(entry.state.lifecycle_phase);
            guard.generations.entry(lease_key.clone()).or_insert(0);
            guard.authorities.remove(lease_key);
            guard.credential_published_at_millis.remove(lease_key);
            (from_phase, to_phase)
        };
        emit_audit(lease_key, "release_lease", from_phase, to_phase);
        #[cfg(test)]
        run_release_after_accept_hook(lease_key);
        #[cfg(not(target_arch = "wasm32"))]
        self.notify_release_observers(&released)?;
        Ok(())
    }

    fn release_credential_lifecycle(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let context = "AuthLeaseHandle::release_credential_lifecycle";
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (input, remove_after_accept, from_phase, to_phase) = {
            let entry =
                guard
                    .authorities
                    .entry(lease_key.clone())
                    .or_insert_with(|| {
                        auth_dsl::AuthMachineAuthority::from_state(
                            auth_dsl::AuthMachineState::default(),
                        )
                    });
            let has_oauth_membership = !entry.state.oauth_browser_flow_ids.is_empty()
                || !entry.state.oauth_device_flow_ids.is_empty()
                || !entry.state.oauth_device_poll_ids.is_empty();
            let input = if has_oauth_membership {
                auth_dsl::AuthMachineInput::ClearCredentialLifecycle
            } else {
                auth_dsl::AuthMachineInput::Release
            };
            let remove_after_accept = matches!(&input, auth_dsl::AuthMachineInput::Release);
            let from_phase = map_phase(entry.state.lifecycle_phase);
            auth_dsl::AuthMachineMutator::apply(entry, input.clone())
                .map_err(|err| DslTransitionError::new(context, format!("{err}")))?;
            let to_phase = map_phase(entry.state.lifecycle_phase);
            (input, remove_after_accept, from_phase, to_phase)
        };
        guard.credential_published_at_millis.remove(lease_key);
        if remove_after_accept {
            guard.authorities.remove(lease_key);
        }
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
        lease_key: &LeaseKey,
        snapshot: &AuthLeaseSnapshot,
        expires_at: Option<u64>,
    ) -> Result<(), DslTransitionError> {
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current_generation = guard.generations.get(lease_key).copied().unwrap_or(0);
        let from_phase = guard
            .authorities
            .get(lease_key)
            .map(|authority| map_phase(authority.state.lifecycle_phase))
            .unwrap_or(AuthLeasePhase::Released);
        let existing_oauth = guard
            .authorities
            .get(lease_key)
            .map(|authority| authority.state.clone());
        let has_oauth_membership = existing_oauth.as_ref().is_some_and(|state| {
            !state.oauth_browser_flow_ids.is_empty()
                || !state.oauth_device_flow_ids.is_empty()
                || !state.oauth_device_poll_ids.is_empty()
        });

        if !snapshot.credential_present
            || snapshot.phase.is_none()
            || snapshot.phase == Some(AuthLeasePhase::Released)
        {
            guard.credential_published_at_millis.remove(lease_key);
            if has_oauth_membership {
                let oauth = existing_oauth.unwrap_or_default();
                guard.authorities.insert(
                    lease_key.clone(),
                    auth_dsl::AuthMachineAuthority::from_state(auth_dsl::AuthMachineState {
                        lifecycle_phase: auth_dsl::AuthLifecyclePhase::ReauthRequired,
                        credential_present: false,
                        oauth_browser_flow_ids: oauth.oauth_browser_flow_ids,
                        oauth_browser_flow_providers: oauth.oauth_browser_flow_providers,
                        oauth_browser_flow_redirect_uris: oauth.oauth_browser_flow_redirect_uris,
                        oauth_browser_flow_expires_at_millis: oauth
                            .oauth_browser_flow_expires_at_millis,
                        oauth_device_flow_ids: oauth.oauth_device_flow_ids,
                        oauth_device_flow_providers: oauth.oauth_device_flow_providers,
                        oauth_device_flow_expires_at_millis: oauth
                            .oauth_device_flow_expires_at_millis,
                        oauth_device_poll_ids: oauth.oauth_device_poll_ids,
                        oauth_outstanding_flow_count: oauth.oauth_outstanding_flow_count,
                        ..Default::default()
                    }),
                );
                guard.generations.insert(
                    lease_key.clone(),
                    current_generation.max(snapshot.generation),
                );
            } else {
                guard.authorities.remove(lease_key);
                if snapshot.generation == 0 {
                    guard.generations.remove(lease_key);
                } else {
                    guard
                        .generations
                        .insert(lease_key.clone(), snapshot.generation);
                }
            }
            emit_audit(
                lease_key,
                "restore_auth_lifecycle_snapshot",
                from_phase,
                AuthLeasePhase::ReauthRequired,
            );
            return Ok(());
        }

        let Some(phase) = snapshot.phase else {
            return Ok(());
        };
        let Some(expires_at) = expires_at else {
            return Ok(());
        };
        let expires_at = if expires_at == u64::MAX {
            None
        } else {
            Some(expires_at)
        };
        let oauth = existing_oauth.unwrap_or_default();
        guard.authorities.insert(
            lease_key.clone(),
            auth_dsl::AuthMachineAuthority::from_state(auth_dsl::AuthMachineState {
                lifecycle_phase: restore_phase(phase),
                expires_at,
                credential_present: true,
                oauth_browser_flow_ids: oauth.oauth_browser_flow_ids,
                oauth_browser_flow_providers: oauth.oauth_browser_flow_providers,
                oauth_browser_flow_redirect_uris: oauth.oauth_browser_flow_redirect_uris,
                oauth_browser_flow_expires_at_millis: oauth.oauth_browser_flow_expires_at_millis,
                oauth_device_flow_ids: oauth.oauth_device_flow_ids,
                oauth_device_flow_providers: oauth.oauth_device_flow_providers,
                oauth_device_flow_expires_at_millis: oauth.oauth_device_flow_expires_at_millis,
                oauth_device_poll_ids: oauth.oauth_device_poll_ids,
                oauth_outstanding_flow_count: oauth.oauth_outstanding_flow_count,
                ..Default::default()
            }),
        );
        guard.generations.insert(
            lease_key.clone(),
            current_generation.max(snapshot.generation),
        );
        match snapshot.credential_published_at_millis {
            Some(published_at) => {
                guard
                    .credential_published_at_millis
                    .insert(lease_key.clone(), published_at);
            }
            None => {
                guard.credential_published_at_millis.remove(lease_key);
            }
        }
        emit_audit(
            lease_key,
            "restore_auth_lifecycle_snapshot",
            from_phase,
            phase,
        );
        Ok(())
    }

    fn snapshot(&self, lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        let guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = guard.generations.get(lease_key).copied().unwrap_or(0);
        match guard.authorities.get(lease_key) {
            Some(machine) => AuthLeaseSnapshot {
                phase: Some(map_phase(machine.state.lifecycle_phase)),
                expires_at: machine.state.expires_at,
                credential_present: machine.state.credential_present,
                generation,
                credential_published_at_millis: machine
                    .state
                    .credential_present
                    .then(|| guard.credential_published_at_millis.get(lease_key).copied())
                    .flatten(),
            },
            None => AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                credential_present: false,
                generation,
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
        set_release_after_accept_hook_for_test(Some(Arc::new(move |released_key| {
            if released_key != &hook_key {
                return;
            }
            let generation = acquire_handle
                .acquire_lease(&acquire_key, 1_800_000_000)
                .unwrap()
                .generation;
            *hook_generation.lock().unwrap() = Some(generation);
        })));

        h.release_lease(&key).unwrap();
        set_release_after_accept_hook_for_test(None);

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
        assert_eq!(before.phase, Some(AuthLeasePhase::Valid));
        assert!(before.credential_present);
        assert!(before.credential_published_at_millis.is_some());

        h.acquire_lease(&key, 1_900_000_000).unwrap();
        let advanced = h.snapshot(&key);
        assert!(advanced.generation > before.generation);

        h.restore_auth_lifecycle_snapshot(&key, &before, before.expires_at)
            .unwrap();

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
    fn restore_empty_zero_generation_snapshot_clears_runtime_authority() {
        let h = RuntimeAuthLeaseHandle::new();
        let key = lease("dev", "shared");
        h.acquire_lease(&key, 1_800_000_000).unwrap();

        h.restore_auth_lifecycle_snapshot(
            &key,
            &AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                credential_present: false,
                generation: 0,
                credential_published_at_millis: None,
            },
            None,
        )
        .unwrap();

        let restored = h.snapshot(&key);
        assert_eq!(restored.phase, None);
        assert_eq!(restored.expires_at, None);
        assert!(!restored.credential_present);
        assert_eq!(restored.generation, 0);
        assert_eq!(restored.credential_published_at_millis, None);
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
