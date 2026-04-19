//! Runtime impl of [`meerkat_core::handles::AuthLeaseHandle`].
//!
//! Per-binding [`AuthMachine`](crate::auth_machine) instances, keyed by
//! `binding_key` (format `"<realm_id>:<binding_id>"`). Each trait
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

use meerkat_core::handles::{AuthLeaseHandle, AuthLeaseSnapshot, DslTransitionError};

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
    binding_key: &str,
    action: &'static str,
    from_phase: Option<&str>,
    to_phase: Option<&str>,
) {
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = %binding_key,
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
    machines: Arc<Mutex<HashMap<String, auth_dsl::AuthMachineAuthority>>>,
}

impl std::fmt::Debug for RuntimeAuthLeaseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.debug_struct("RuntimeAuthLeaseHandle")
            .field("bindings", &guard.keys().collect::<Vec<_>>())
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
        binding_key: &str,
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
            guard.entry(binding_key.to_string()).or_insert_with(|| {
                auth_dsl::AuthMachineAuthority::from_state(auth_dsl::AuthMachineState::default())
            })
        } else {
            match guard.get_mut(binding_key) {
                Some(m) => m,
                None => {
                    return Err(DslTransitionError::new(
                        context,
                        format!("no auth lease registered for binding_key `{binding_key}`"),
                    ));
                }
            }
        };
        let from_phase = Self::phase_str(&entry.state.lifecycle_phase);
        auth_dsl::AuthMachineMutator::apply(entry, input)
            .map_err(|err| DslTransitionError::new(context, format!("{err}")))?;
        let to_phase = Self::phase_str(&entry.state.lifecycle_phase);
        emit_audit(binding_key, action, Some(from_phase), Some(to_phase));
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

    fn phase_str(phase: &auth_dsl::AuthLifecyclePhase) -> &'static str {
        match phase {
            auth_dsl::AuthLifecyclePhase::Valid => "valid",
            auth_dsl::AuthLifecyclePhase::Expiring => "expiring",
            auth_dsl::AuthLifecyclePhase::Refreshing => "refreshing",
            auth_dsl::AuthLifecyclePhase::ReauthRequired => "reauth_required",
            auth_dsl::AuthLifecyclePhase::Released => "released",
        }
    }
}

impl Default for RuntimeAuthLeaseHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthLeaseHandle for RuntimeAuthLeaseHandle {
    fn acquire_lease(&self, binding_key: &str, expires_at: u64) -> Result<(), DslTransitionError> {
        let expires_at_ts = if expires_at == u64::MAX {
            None
        } else {
            Some(expires_at)
        };
        self.apply(
            binding_key,
            auth_dsl::AuthMachineInput::Acquire { expires_at_ts },
            "AuthLeaseHandle::acquire_lease",
            true,
        )
    }

    fn mark_expiring(&self, binding_key: &str) -> Result<(), DslTransitionError> {
        self.apply(
            binding_key,
            auth_dsl::AuthMachineInput::MarkExpiring,
            "AuthLeaseHandle::mark_expiring",
            false,
        )
    }

    fn begin_refresh(&self, binding_key: &str) -> Result<(), DslTransitionError> {
        self.apply(
            binding_key,
            auth_dsl::AuthMachineInput::BeginRefresh,
            "AuthLeaseHandle::begin_refresh",
            false,
        )
    }

    fn complete_refresh(
        &self,
        binding_key: &str,
        new_expires_at: u64,
        now: u64,
    ) -> Result<(), DslTransitionError> {
        let new_expires_at = if new_expires_at == u64::MAX {
            None
        } else {
            Some(new_expires_at)
        };
        self.apply(
            binding_key,
            auth_dsl::AuthMachineInput::CompleteRefresh {
                new_expires_at,
                now_ts: now,
            },
            "AuthLeaseHandle::complete_refresh",
            false,
        )
    }

    fn refresh_failed(&self, binding_key: &str, permanent: bool) -> Result<(), DslTransitionError> {
        let input = if permanent {
            auth_dsl::AuthMachineInput::RefreshFailedPermanent
        } else {
            auth_dsl::AuthMachineInput::RefreshFailedTransient
        };
        self.apply(binding_key, input, "AuthLeaseHandle::refresh_failed", false)
    }

    fn mark_reauth_required(&self, binding_key: &str) -> Result<(), DslTransitionError> {
        self.apply(
            binding_key,
            auth_dsl::AuthMachineInput::MarkReauthRequired,
            "AuthLeaseHandle::mark_reauth_required",
            false,
        )
    }

    fn release_lease(&self, binding_key: &str) -> Result<(), DslTransitionError> {
        // Drive the DSL transition, then drop the machine from the
        // registry: a released lease has no observable state and
        // keeping the spent AuthMachine around is shell-side bookkeeping
        // that dogma §14 forbids (local state, no canonical role).
        self.apply(
            binding_key,
            auth_dsl::AuthMachineInput::Release,
            "AuthLeaseHandle::release_lease",
            false,
        )?;
        let mut guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.remove(binding_key);
        Ok(())
    }

    fn snapshot(&self, binding_key: &str) -> AuthLeaseSnapshot {
        let guard = self
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.get(binding_key) {
            Some(machine) => {
                let state = &machine.state;
                let phase_str = match state.lifecycle_phase {
                    auth_dsl::AuthLifecyclePhase::Valid => Some("valid".to_string()),
                    auth_dsl::AuthLifecyclePhase::Expiring => Some("expiring".to_string()),
                    auth_dsl::AuthLifecyclePhase::Refreshing => Some("refreshing".to_string()),
                    auth_dsl::AuthLifecyclePhase::ReauthRequired => {
                        Some("reauth_required".to_string())
                    }
                    auth_dsl::AuthLifecyclePhase::Released => Some("released".to_string()),
                };
                AuthLeaseSnapshot {
                    state: phase_str,
                    expires_at: state.expires_at,
                }
            }
            None => AuthLeaseSnapshot {
                state: None,
                expires_at: None,
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_snapshot_roundtrip() {
        let h = RuntimeAuthLeaseHandle::new();
        h.acquire_lease("dev:default_openai", 1_800_000_000)
            .unwrap();
        let snap = h.snapshot("dev:default_openai");
        assert_eq!(snap.state.as_deref(), Some("valid"));
        assert_eq!(snap.expires_at, Some(1_800_000_000));
    }

    #[test]
    fn lifecycle_transitions() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = "dev:default_anthropic";

        h.acquire_lease(k, 1_800_000_000).unwrap();
        assert_eq!(h.snapshot(k).state.as_deref(), Some("valid"));

        h.mark_expiring(k).unwrap();
        assert_eq!(h.snapshot(k).state.as_deref(), Some("expiring"));

        h.begin_refresh(k).unwrap();
        assert_eq!(h.snapshot(k).state.as_deref(), Some("refreshing"));

        h.complete_refresh(k, 1_800_000_900, 1_800_000_000).unwrap();
        let snap = h.snapshot(k);
        assert_eq!(snap.state.as_deref(), Some("valid"));
        assert_eq!(snap.expires_at, Some(1_800_000_900));
    }

    #[test]
    fn refresh_failed_permanent_routes_to_reauth() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = "dev:default_google";

        h.acquire_lease(k, 1_800_000_000).unwrap();
        h.begin_refresh(k).unwrap();
        h.refresh_failed(k, true).unwrap();

        assert_eq!(h.snapshot(k).state.as_deref(), Some("reauth_required"));
    }

    #[test]
    fn refresh_failed_transient_routes_to_expiring() {
        let h = RuntimeAuthLeaseHandle::new();
        let k = "dev:foo";

        h.acquire_lease(k, 1_800_000_000).unwrap();
        h.begin_refresh(k).unwrap();
        h.refresh_failed(k, false).unwrap();

        assert_eq!(h.snapshot(k).state.as_deref(), Some("expiring"));
    }

    #[test]
    fn snapshot_for_unknown_binding_is_none() {
        let h = RuntimeAuthLeaseHandle::new();
        let snap = h.snapshot("dev:never_registered");
        assert!(snap.state.is_none());
        assert!(snap.expires_at.is_none());
    }

    #[test]
    fn mark_expiring_before_acquire_errors() {
        let h = RuntimeAuthLeaseHandle::new();
        let err = h.mark_expiring("dev:ghost").unwrap_err();
        assert_eq!(err.context, "AuthLeaseHandle::mark_expiring");
    }

    #[test]
    fn per_binding_isolation() {
        let h = RuntimeAuthLeaseHandle::new();
        h.acquire_lease("dev:openai", 1_800_000_000).unwrap();
        h.acquire_lease("dev:anthropic", 1_900_000_000).unwrap();
        h.mark_expiring("dev:openai").unwrap();

        assert_eq!(h.snapshot("dev:openai").state.as_deref(), Some("expiring"));
        assert_eq!(h.snapshot("dev:anthropic").state.as_deref(), Some("valid"));
        assert_eq!(h.snapshot("dev:anthropic").expires_at, Some(1_900_000_000));
    }
}
