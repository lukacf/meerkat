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
//   * short-lived OAuth login-flow membership for the same binding:
//     browser PKCE states, admitted device codes, and active device
//     poll leases
//
// Per-binding (rather than one machine with multi-binding state) keeps
// the TLC state space small, aligns the machine with dogma §1 "one
// semantic fact, one owner" (each lease IS a distinct fact), and lets
// auth be orthogonal to MeerkatMachine — which is what Luka flagged
// when reviewing the absorbed design.
#[macro_export]
macro_rules! auth_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        meerkat_machine_dsl::machine! {
        machine AuthMachine {
            version: 1,
            rust: $rust_crate / $rust_module,

            state {
                lifecycle_phase: AuthLifecyclePhase,
                expires_at: Option<u64>,
                last_refresh: Option<u64>,
                refresh_attempt: u64,
                oauth_browser_flow_ids: Set<String>,
                oauth_device_flow_ids: Set<String>,
                oauth_device_poll_ids: Set<String>,
            }

            init(Valid) {
                expires_at = None,
                last_refresh = None,
                refresh_attempt = 0,
                oauth_browser_flow_ids = EmptySet,
                oauth_device_flow_ids = EmptySet,
                oauth_device_poll_ids = EmptySet,
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
                AdmitOAuthBrowserFlow { flow_id: String },
                VerifyOAuthBrowserFlow { flow_id: String },
                ConsumeOAuthBrowserFlow { flow_id: String },
                ExpireOAuthBrowserFlow { flow_id: String },
                AdmitOAuthDeviceFlow { flow_id: String },
                VerifyOAuthDeviceFlow { flow_id: String },
                BeginOAuthDevicePoll { flow_id: String },
                FinishOAuthDevicePoll { flow_id: String },
                ConsumeOAuthDeviceFlow { flow_id: String },
                ExpireOAuthDeviceFlow { flow_id: String },
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
                update {
                    self.oauth_browser_flow_ids = EmptySet;
                    self.oauth_device_flow_ids = EmptySet;
                    self.oauth_device_poll_ids = EmptySet;
                }
                to Released
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            // `per_phase` expands each OAuth flow transition into one
            // transition per listed phase and rewrites the target to that same
            // phase. The syntactic `to Valid` below is therefore only a grammar
            // placeholder; OAuth membership changes must not manufacture a
            // credential-valid lease.
            transition AdmitOAuthBrowserFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input AdmitOAuthBrowserFlow { flow_id }
                guard "browser_flow_absent" { self.oauth_browser_flow_ids.contains(flow_id) == false }
                update {
                    self.oauth_browser_flow_ids.insert(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition VerifyOAuthBrowserFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input VerifyOAuthBrowserFlow { flow_id }
                guard "browser_flow_present" { self.oauth_browser_flow_ids.contains(flow_id) }
                update {}
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition ConsumeOAuthBrowserFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input ConsumeOAuthBrowserFlow { flow_id }
                guard "browser_flow_present" { self.oauth_browser_flow_ids.contains(flow_id) }
                update {
                    self.oauth_browser_flow_ids.remove(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition ExpireOAuthBrowserFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input ExpireOAuthBrowserFlow { flow_id }
                guard "browser_flow_present" { self.oauth_browser_flow_ids.contains(flow_id) }
                update {
                    self.oauth_browser_flow_ids.remove(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition AdmitOAuthDeviceFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input AdmitOAuthDeviceFlow { flow_id }
                guard "device_flow_absent" { self.oauth_device_flow_ids.contains(flow_id) == false }
                update {
                    self.oauth_device_flow_ids.insert(flow_id);
                    self.oauth_device_poll_ids.remove(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition VerifyOAuthDeviceFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input VerifyOAuthDeviceFlow { flow_id }
                guard "device_flow_present" { self.oauth_device_flow_ids.contains(flow_id) }
                update {}
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition BeginOAuthDevicePoll {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input BeginOAuthDevicePoll { flow_id }
                guard "device_flow_present" { self.oauth_device_flow_ids.contains(flow_id) }
                guard "device_poll_absent" { self.oauth_device_poll_ids.contains(flow_id) == false }
                update {
                    self.oauth_device_poll_ids.insert(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition FinishOAuthDevicePoll {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input FinishOAuthDevicePoll { flow_id }
                guard "device_poll_present" { self.oauth_device_poll_ids.contains(flow_id) }
                update {
                    self.oauth_device_poll_ids.remove(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition ConsumeOAuthDeviceFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input ConsumeOAuthDeviceFlow { flow_id }
                guard "device_flow_present" { self.oauth_device_flow_ids.contains(flow_id) }
                update {
                    self.oauth_device_flow_ids.remove(flow_id);
                    self.oauth_device_poll_ids.remove(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition ExpireOAuthDeviceFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input ExpireOAuthDeviceFlow { flow_id }
                guard "device_flow_present" { self.oauth_device_flow_ids.contains(flow_id) }
                update {
                    self.oauth_device_flow_ids.remove(flow_id);
                    self.oauth_device_poll_ids.remove(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }
        }
            }
    };
}

crate::auth_catalog_machine_dsl!("self", "catalog::dsl::auth_machine");
