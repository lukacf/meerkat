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
use std::sync::{Arc, Mutex};

use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, DslTransitionError, LeaseKey,
};

use crate::auth_machine::dsl as auth_dsl;

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
/// instances. Lookup-or-insert happens on first `acquire_lease`; all
/// other operations require the binding to already exist.
pub struct RuntimeAuthLeaseHandle {
    machines: Arc<Mutex<HashMap<LeaseKey, auth_dsl::AuthMachineAuthority>>>,
}

impl std::fmt::Debug for RuntimeAuthLeaseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.debug_struct("RuntimeAuthLeaseHandle")
            .field("leases", &guard.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RuntimeAuthLeaseHandle {
    pub fn new() -> Self {
        Self {
            machines: Arc::new(Mutex::new(HashMap::new())),
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

    fn apply(
        &self,
        lease_key: &LeaseKey,
        input: auth_dsl::AuthMachineInput,
        context: &'static str,
        create_if_missing: bool,
    ) -> Result<(), DslTransitionError> {
        let action = Self::audit_action_for(&input);
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = if create_if_missing {
            guard.entry(lease_key.clone()).or_insert_with(|| {
                auth_dsl::AuthMachineAuthority::from_state(auth_dsl::AuthMachineState::default())
            })
        } else {
            match guard.get_mut(lease_key) {
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
        emit_audit(lease_key, action, from_phase, to_phase);
        Ok(())
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
            auth_dsl::AuthMachineInput::Release => "release_lease",
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
    ) -> Result<(), DslTransitionError> {
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
    }

    fn begin_refresh(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::BeginRefresh,
            "AuthLeaseHandle::begin_refresh",
            false,
        )
    }

    fn complete_refresh(
        &self,
        lease_key: &LeaseKey,
        new_expires_at: u64,
        now: u64,
    ) -> Result<(), DslTransitionError> {
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
    }

    fn mark_reauth_required(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::MarkReauthRequired,
            "AuthLeaseHandle::mark_reauth_required",
            false,
        )
    }

    fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        // Drive the DSL transition, then drop the machine from the
        // registry: a released lease has no observable state and
        // keeping the spent AuthMachine around is shell-side bookkeeping
        // that dogma §14 forbids (local state, no canonical role).
        self.apply(
            lease_key,
            auth_dsl::AuthMachineInput::Release,
            "AuthLeaseHandle::release_lease",
            false,
        )?;
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.remove(lease_key);
        Ok(())
    }

    fn snapshot(&self, lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        let guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.get(lease_key) {
            Some(machine) => AuthLeaseSnapshot {
                phase: Some(map_phase(machine.state.lifecycle_phase)),
                expires_at: machine.state.expires_at,
            },
            None => AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
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
