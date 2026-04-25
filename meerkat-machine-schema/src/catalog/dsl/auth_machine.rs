use meerkat_machine_dsl::machine;

// AuthMachine — per-binding auth lease lifecycle (Phase 1.5-rev,
// refactored from the original "absorbed into MeerkatMachine"
// design after review).
//
// Each binding_key (format: "<realm_id>:<binding_id>") has its own
// AuthMachine instance, tracked by the runtime-level registry in
// `meerkat-runtime/src/handles/auth_lease.rs`. The machine owns the
// semantics of:
//
//   * whether a lease is currently fresh / expiring / refreshing /
//     requires reauth / released
//   * the legal transitions between those states (e.g. refresh only
//     from Valid or Expiring; complete only from Refreshing)
//   * the expiry timestamp, last refresh timestamp, and consecutive
//     refresh-failure count associated with the lease
//
// Per-binding (rather than one machine with multi-binding state) keeps
// the TLC state space small, aligns the machine with dogma §1 "one
// semantic fact, one owner" (each lease IS a distinct fact), and lets
// auth be orthogonal to MeerkatMachine — which is what Luka flagged
// when reviewing the absorbed design.
machine! {
    machine AuthMachine {
        version: 1,
        rust: "self" / "catalog::dsl::auth_machine",

        state {
            lifecycle_phase: AuthLifecyclePhase,
            expires_at: Option<u64>,
            last_refresh: Option<u64>,
            refresh_attempt: u64,
        }

        init(Valid) {
            expires_at = None,
            last_refresh = None,
            refresh_attempt = 0,
        }

        terminal [Released]

        phase AuthLifecyclePhase {
            Valid,
            Expiring,
            Refreshing,
            ReauthRequired,
            Released,
        }

        input AuthMachineInput {
            Acquire { expires_at_ts: Option<u64> },
            MarkExpiring,
            BeginRefresh,
            CompleteRefresh { new_expires_at: Option<u64>, now_ts: u64 },
            RefreshFailedTransient,
            RefreshFailedPermanent,
            MarkReauthRequired,
            Release,
        }

        effect AuthMachineEffect {
            EmitLifecycleEvent { new_state: AuthLifecyclePhase },
            WakeRefreshLoop,
        }

        disposition EmitLifecycleEvent => external handoff auth_lease_lifecycle_publication,
        disposition WakeRefreshLoop => local,

        // --- Transitions ---

        transition Acquire {
            on input Acquire { expires_at_ts }
            update {
                self.expires_at = expires_at_ts;
                self.refresh_attempt = 0;
            }
            to Valid
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }

        transition MarkExpiring {
            on input MarkExpiring
            guard { self.lifecycle_phase == Phase::Valid }
            to Expiring
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }

        transition BeginRefreshFromValid {
            on input BeginRefresh
            guard { self.lifecycle_phase == Phase::Valid }
            to Refreshing
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            emit WakeRefreshLoop
        }

        transition BeginRefreshFromExpiring {
            on input BeginRefresh
            guard { self.lifecycle_phase == Phase::Expiring }
            to Refreshing
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            emit WakeRefreshLoop
        }

        transition CompleteRefresh {
            on input CompleteRefresh { new_expires_at, now_ts }
            guard { self.lifecycle_phase == Phase::Refreshing }
            update {
                self.expires_at = new_expires_at;
                self.last_refresh = Some(now_ts);
                self.refresh_attempt = 0;
            }
            to Valid
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }

        transition RefreshFailedTransient {
            on input RefreshFailedTransient
            guard { self.lifecycle_phase == Phase::Refreshing }
            update {
                self.refresh_attempt = self.refresh_attempt + 1;
            }
            to Expiring
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }

        transition RefreshFailedPermanent {
            on input RefreshFailedPermanent
            guard { self.lifecycle_phase == Phase::Refreshing }
            update {
                self.refresh_attempt = self.refresh_attempt + 1;
            }
            to ReauthRequired
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }

        transition MarkReauthRequiredFromValid {
            on input MarkReauthRequired
            guard { self.lifecycle_phase == Phase::Valid }
            to ReauthRequired
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }

        transition MarkReauthRequiredFromExpiring {
            on input MarkReauthRequired
            guard { self.lifecycle_phase == Phase::Expiring }
            to ReauthRequired
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }

        transition MarkReauthRequiredFromRefreshing {
            on input MarkReauthRequired
            guard { self.lifecycle_phase == Phase::Refreshing }
            to ReauthRequired
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }

        transition Release {
            on input Release
            to Released
            emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
        }
    }
}
