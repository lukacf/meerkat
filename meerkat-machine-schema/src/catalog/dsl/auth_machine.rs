// AuthMachine — per-binding auth lease lifecycle (Phase 1.5-rev,
// refactored from the original "absorbed into MeerkatMachine"
// design after review).
use super::OptionValueExt;
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
                credential_present: bool,
                oauth_browser_flow_ids: Set<String>,
                oauth_browser_flow_providers: Map<String, String>,
                oauth_browser_flow_redirect_uris: Map<String, String>,
                oauth_browser_flow_expires_at_millis: Map<String, u64>,
                oauth_device_flow_ids: Set<String>,
                oauth_device_flow_providers: Map<String, String>,
                oauth_device_flow_expires_at_millis: Map<String, u64>,
                oauth_device_poll_ids: Set<String>,
                oauth_outstanding_flow_count: u64,
            }

            init(Valid) {
                expires_at = None,
                last_refresh = None,
                refresh_attempt = 0,
                credential_present = false,
                oauth_browser_flow_ids = EmptySet,
                oauth_browser_flow_providers = EmptyMap,
                oauth_browser_flow_redirect_uris = EmptyMap,
                oauth_browser_flow_expires_at_millis = EmptyMap,
                oauth_device_flow_ids = EmptySet,
                oauth_device_flow_providers = EmptyMap,
                oauth_device_flow_expires_at_millis = EmptyMap,
                oauth_device_poll_ids = EmptySet,
                oauth_outstanding_flow_count = 0,
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
                ClearCredentialLifecycle,
                Release,
                RestoreAuthoritySnapshot {
                    lifecycle_phase: AuthLifecyclePhase,
                    expires_at: Option<u64>,
                    last_refresh: Option<u64>,
                    refresh_attempt: u64,
                    credential_present: bool,
                    oauth_browser_flow_ids: Set<String>,
                    oauth_browser_flow_providers: Map<String, String>,
                    oauth_browser_flow_redirect_uris: Map<String, String>,
                    oauth_browser_flow_expires_at_millis: Map<String, u64>,
                    oauth_device_flow_ids: Set<String>,
                    oauth_device_flow_providers: Map<String, String>,
                    oauth_device_flow_expires_at_millis: Map<String, u64>,
                    oauth_device_poll_ids: Set<String>,
                    oauth_outstanding_flow_count: u64,
                },
                AdmitOAuthBrowserFlow {
                    flow_id: String,
                    provider: String,
                    redirect_uri: String,
                    expires_at_millis: u64,
                    max_outstanding_flows: u64,
                    observed_global_outstanding_flows: u64,
                },
                VerifyOAuthBrowserFlow { flow_id: String, provider: String, redirect_uri: String, now_millis: u64 },
                ConsumeOAuthBrowserFlow { flow_id: String, provider: String, redirect_uri: String, now_millis: u64 },
                ExpireOAuthBrowserFlow { flow_id: String },
                AdmitOAuthDeviceFlow {
                    flow_id: String,
                    provider: String,
                    expires_at_millis: u64,
                    max_outstanding_flows: u64,
                    observed_global_outstanding_flows: u64,
                },
                ConfirmOAuthDurableAdmission {
                    observed_global_outstanding_flows: u64,
                    max_outstanding_flows: u64,
                },
                VerifyOAuthDeviceFlow { flow_id: String, provider: String, now_millis: u64 },
                BeginOAuthDevicePoll { flow_id: String, provider: String, now_millis: u64 },
                FinishOAuthDevicePoll { flow_id: String },
                ConsumeOAuthDeviceFlow { flow_id: String, provider: String, now_millis: u64 },
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
                    self.credential_present = true;
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

            transition ClearCredentialLifecycle {
                on input ClearCredentialLifecycle
                update {
                    self.expires_at = None;
                    self.last_refresh = None;
                    self.refresh_attempt = 0;
                    self.credential_present = false;
                }
                to ReauthRequired
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition Release {
                on input Release
                update {
                    self.credential_present = false;
                    self.oauth_browser_flow_ids = EmptySet;
                    self.oauth_browser_flow_providers = EmptyMap;
                    self.oauth_browser_flow_redirect_uris = EmptyMap;
                    self.oauth_browser_flow_expires_at_millis = EmptyMap;
                    self.oauth_device_flow_ids = EmptySet;
                    self.oauth_device_flow_providers = EmptyMap;
                    self.oauth_device_flow_expires_at_millis = EmptyMap;
                    self.oauth_device_poll_ids = EmptySet;
                    self.oauth_outstanding_flow_count = 0;
                }
                to Released
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition RestoreAuthoritySnapshotValid {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    oauth_browser_flow_ids,
                    oauth_browser_flow_providers,
                    oauth_browser_flow_redirect_uris,
                    oauth_browser_flow_expires_at_millis,
                    oauth_device_flow_ids,
                    oauth_device_flow_providers,
                    oauth_device_flow_expires_at_millis,
                    oauth_device_poll_ids,
                    oauth_outstanding_flow_count
                }
                guard { lifecycle_phase == Phase::Valid && credential_present }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    self.oauth_browser_flow_ids = oauth_browser_flow_ids;
                    self.oauth_browser_flow_providers = oauth_browser_flow_providers;
                    self.oauth_browser_flow_redirect_uris = oauth_browser_flow_redirect_uris;
                    self.oauth_browser_flow_expires_at_millis = oauth_browser_flow_expires_at_millis;
                    self.oauth_device_flow_ids = oauth_device_flow_ids;
                    self.oauth_device_flow_providers = oauth_device_flow_providers;
                    self.oauth_device_flow_expires_at_millis = oauth_device_flow_expires_at_millis;
                    self.oauth_device_poll_ids = oauth_device_poll_ids;
                    self.oauth_outstanding_flow_count = oauth_outstanding_flow_count;
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition RestoreAuthoritySnapshotExpiring {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    oauth_browser_flow_ids,
                    oauth_browser_flow_providers,
                    oauth_browser_flow_redirect_uris,
                    oauth_browser_flow_expires_at_millis,
                    oauth_device_flow_ids,
                    oauth_device_flow_providers,
                    oauth_device_flow_expires_at_millis,
                    oauth_device_poll_ids,
                    oauth_outstanding_flow_count
                }
                guard { lifecycle_phase == Phase::Expiring && credential_present }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    self.oauth_browser_flow_ids = oauth_browser_flow_ids;
                    self.oauth_browser_flow_providers = oauth_browser_flow_providers;
                    self.oauth_browser_flow_redirect_uris = oauth_browser_flow_redirect_uris;
                    self.oauth_browser_flow_expires_at_millis = oauth_browser_flow_expires_at_millis;
                    self.oauth_device_flow_ids = oauth_device_flow_ids;
                    self.oauth_device_flow_providers = oauth_device_flow_providers;
                    self.oauth_device_flow_expires_at_millis = oauth_device_flow_expires_at_millis;
                    self.oauth_device_poll_ids = oauth_device_poll_ids;
                    self.oauth_outstanding_flow_count = oauth_outstanding_flow_count;
                }
                to Expiring
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition RestoreAuthoritySnapshotRefreshing {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    oauth_browser_flow_ids,
                    oauth_browser_flow_providers,
                    oauth_browser_flow_redirect_uris,
                    oauth_browser_flow_expires_at_millis,
                    oauth_device_flow_ids,
                    oauth_device_flow_providers,
                    oauth_device_flow_expires_at_millis,
                    oauth_device_poll_ids,
                    oauth_outstanding_flow_count
                }
                guard { lifecycle_phase == Phase::Refreshing && credential_present }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    self.oauth_browser_flow_ids = oauth_browser_flow_ids;
                    self.oauth_browser_flow_providers = oauth_browser_flow_providers;
                    self.oauth_browser_flow_redirect_uris = oauth_browser_flow_redirect_uris;
                    self.oauth_browser_flow_expires_at_millis = oauth_browser_flow_expires_at_millis;
                    self.oauth_device_flow_ids = oauth_device_flow_ids;
                    self.oauth_device_flow_providers = oauth_device_flow_providers;
                    self.oauth_device_flow_expires_at_millis = oauth_device_flow_expires_at_millis;
                    self.oauth_device_poll_ids = oauth_device_poll_ids;
                    self.oauth_outstanding_flow_count = oauth_outstanding_flow_count;
                }
                to Refreshing
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition RestoreAuthoritySnapshotReauthRequired {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    oauth_browser_flow_ids,
                    oauth_browser_flow_providers,
                    oauth_browser_flow_redirect_uris,
                    oauth_browser_flow_expires_at_millis,
                    oauth_device_flow_ids,
                    oauth_device_flow_providers,
                    oauth_device_flow_expires_at_millis,
                    oauth_device_poll_ids,
                    oauth_outstanding_flow_count
                }
                guard { lifecycle_phase == Phase::ReauthRequired }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    self.oauth_browser_flow_ids = oauth_browser_flow_ids;
                    self.oauth_browser_flow_providers = oauth_browser_flow_providers;
                    self.oauth_browser_flow_redirect_uris = oauth_browser_flow_redirect_uris;
                    self.oauth_browser_flow_expires_at_millis = oauth_browser_flow_expires_at_millis;
                    self.oauth_device_flow_ids = oauth_device_flow_ids;
                    self.oauth_device_flow_providers = oauth_device_flow_providers;
                    self.oauth_device_flow_expires_at_millis = oauth_device_flow_expires_at_millis;
                    self.oauth_device_poll_ids = oauth_device_poll_ids;
                    self.oauth_outstanding_flow_count = oauth_outstanding_flow_count;
                }
                to ReauthRequired
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition RestoreAuthoritySnapshotReleased {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    oauth_browser_flow_ids,
                    oauth_browser_flow_providers,
                    oauth_browser_flow_redirect_uris,
                    oauth_browser_flow_expires_at_millis,
                    oauth_device_flow_ids,
                    oauth_device_flow_providers,
                    oauth_device_flow_expires_at_millis,
                    oauth_device_poll_ids,
                    oauth_outstanding_flow_count
                }
                guard { lifecycle_phase == Phase::Released && credential_present == false && oauth_outstanding_flow_count == 0 }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    self.oauth_browser_flow_ids = oauth_browser_flow_ids;
                    self.oauth_browser_flow_providers = oauth_browser_flow_providers;
                    self.oauth_browser_flow_redirect_uris = oauth_browser_flow_redirect_uris;
                    self.oauth_browser_flow_expires_at_millis = oauth_browser_flow_expires_at_millis;
                    self.oauth_device_flow_ids = oauth_device_flow_ids;
                    self.oauth_device_flow_providers = oauth_device_flow_providers;
                    self.oauth_device_flow_expires_at_millis = oauth_device_flow_expires_at_millis;
                    self.oauth_device_poll_ids = oauth_device_poll_ids;
                    self.oauth_outstanding_flow_count = oauth_outstanding_flow_count;
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
                on input AdmitOAuthBrowserFlow { flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows }
                guard "browser_flow_absent" { self.oauth_browser_flow_ids.contains(flow_id) == false }
                guard "oauth_capacity_available" { self.oauth_outstanding_flow_count < max_outstanding_flows }
                guard "oauth_global_capacity_available" { observed_global_outstanding_flows < max_outstanding_flows }
                update {
                    self.oauth_browser_flow_ids.insert(flow_id);
                    self.oauth_browser_flow_providers.insert(flow_id, provider);
                    self.oauth_browser_flow_redirect_uris.insert(flow_id, redirect_uri);
                    self.oauth_browser_flow_expires_at_millis.insert(flow_id, expires_at_millis);
                    self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count + 1;
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition VerifyOAuthBrowserFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input VerifyOAuthBrowserFlow { flow_id, provider, redirect_uri, now_millis }
                guard "browser_flow_present" { self.oauth_browser_flow_ids.contains(flow_id) }
                guard "browser_flow_provider_matches" { self.oauth_browser_flow_providers.get_cloned(flow_id) == Some(provider) }
                guard "browser_flow_redirect_uri_matches" { self.oauth_browser_flow_redirect_uris.get_cloned(flow_id) == Some(redirect_uri) }
                guard "browser_flow_not_expired" { now_millis <= self.oauth_browser_flow_expires_at_millis.get_cloned(flow_id).get("value") }
                update {}
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition ConsumeOAuthBrowserFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input ConsumeOAuthBrowserFlow { flow_id, provider, redirect_uri, now_millis }
                guard "browser_flow_present" { self.oauth_browser_flow_ids.contains(flow_id) }
                guard "browser_flow_provider_matches" { self.oauth_browser_flow_providers.get_cloned(flow_id) == Some(provider) }
                guard "browser_flow_redirect_uri_matches" { self.oauth_browser_flow_redirect_uris.get_cloned(flow_id) == Some(redirect_uri) }
                guard "browser_flow_not_expired" { now_millis <= self.oauth_browser_flow_expires_at_millis.get_cloned(flow_id).get("value") }
                update {
                    self.oauth_browser_flow_ids.remove(flow_id);
                    self.oauth_browser_flow_providers.remove(flow_id);
                    self.oauth_browser_flow_redirect_uris.remove(flow_id);
                    self.oauth_browser_flow_expires_at_millis.remove(flow_id);
                    self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count - 1;
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
                    self.oauth_browser_flow_providers.remove(flow_id);
                    self.oauth_browser_flow_redirect_uris.remove(flow_id);
                    self.oauth_browser_flow_expires_at_millis.remove(flow_id);
                    self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count - 1;
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition AdmitOAuthDeviceFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input AdmitOAuthDeviceFlow { flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows }
                guard "device_flow_absent" { self.oauth_device_flow_ids.contains(flow_id) == false }
                guard "oauth_capacity_available" { self.oauth_outstanding_flow_count < max_outstanding_flows }
                guard "oauth_global_capacity_available" { observed_global_outstanding_flows < max_outstanding_flows }
                update {
                    self.oauth_device_flow_ids.insert(flow_id);
                    self.oauth_device_flow_providers.insert(flow_id, provider);
                    self.oauth_device_flow_expires_at_millis.insert(flow_id, expires_at_millis);
                    self.oauth_device_poll_ids.remove(flow_id);
                    self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count + 1;
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition ConfirmOAuthDurableAdmission {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input ConfirmOAuthDurableAdmission { observed_global_outstanding_flows, max_outstanding_flows }
                guard "oauth_global_capacity_available" { observed_global_outstanding_flows < max_outstanding_flows }
                update {}
                to Valid
            }

            transition VerifyOAuthDeviceFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input VerifyOAuthDeviceFlow { flow_id, provider, now_millis }
                guard "device_flow_present" { self.oauth_device_flow_ids.contains(flow_id) }
                guard "device_flow_provider_matches" { self.oauth_device_flow_providers.get_cloned(flow_id) == Some(provider) }
                guard "device_flow_not_expired" { now_millis <= self.oauth_device_flow_expires_at_millis.get_cloned(flow_id).get("value") }
                update {}
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }

            transition BeginOAuthDevicePoll {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input BeginOAuthDevicePoll { flow_id, provider, now_millis }
                guard "device_flow_present" { self.oauth_device_flow_ids.contains(flow_id) }
                guard "device_flow_provider_matches" { self.oauth_device_flow_providers.get_cloned(flow_id) == Some(provider) }
                guard "device_flow_not_expired" { now_millis <= self.oauth_device_flow_expires_at_millis.get_cloned(flow_id).get("value") }
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
                on input ConsumeOAuthDeviceFlow { flow_id, provider, now_millis }
                guard "device_flow_present" { self.oauth_device_flow_ids.contains(flow_id) }
                guard "device_flow_provider_matches" { self.oauth_device_flow_providers.get_cloned(flow_id) == Some(provider) }
                guard "device_flow_not_expired" { now_millis <= self.oauth_device_flow_expires_at_millis.get_cloned(flow_id).get("value") }
                update {
                    self.oauth_device_flow_ids.remove(flow_id);
                    self.oauth_device_flow_providers.remove(flow_id);
                    self.oauth_device_flow_expires_at_millis.remove(flow_id);
                    self.oauth_device_poll_ids.remove(flow_id);
                    self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count - 1;
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
                    self.oauth_device_flow_providers.remove(flow_id);
                    self.oauth_device_flow_expires_at_millis.remove(flow_id);
                    self.oauth_device_poll_ids.remove(flow_id);
                    self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count - 1;
                }
                to Valid
                emit EmitLifecycleEvent { new_state: self.lifecycle_phase }
            }
        }
            }
    };
}

crate::auth_catalog_machine_dsl!("self", "catalog::dsl::auth_machine");
