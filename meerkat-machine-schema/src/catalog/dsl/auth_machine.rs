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
//   * the expiry timestamp, last refresh timestamp, credential-publication
//     generation/time, and consecutive refresh-failure count associated
//     with the lease
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
                credential_generation: u64,
                credential_published_at_millis: Option<u64>,
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
                credential_generation = 0,
                credential_published_at_millis = None,
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
                Acquire { expires_at_ts: Option<u64>, credential_published_at_millis: u64 },
                MarkExpiring,
                BeginRefresh,
                CompleteRefresh { new_expires_at: Option<u64>, now_ts: u64, credential_published_at_millis: u64 },
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
                    credential_generation: u64,
                    credential_published_at_millis: Option<u64>,
                },
                RestoreOAuthBrowserFlow { flow_id: String, provider: String, redirect_uri: String, expires_at_millis: u64 },
                RestoreOAuthDeviceFlow { flow_id: String, provider: String, expires_at_millis: u64, poll_active: bool },
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
                EmitLifecycleEvent {
                    new_state: AuthLifecyclePhase,
                    expires_at: Option<u64>,
                    credential_generation: u64,
                    credential_published_at_millis: Option<u64>,
                },
                WakeRefreshLoop,
            }

            disposition EmitLifecycleEvent => external handoff auth_lease_lifecycle_publication,
            disposition WakeRefreshLoop => local,

            invariant oauth_flow_membership_consistent {
                self.oauth_browser_flow_providers.keys() == self.oauth_browser_flow_ids
                && self.oauth_browser_flow_redirect_uris.keys() == self.oauth_browser_flow_ids
                && self.oauth_browser_flow_expires_at_millis.keys() == self.oauth_browser_flow_ids
                && self.oauth_device_flow_providers.keys() == self.oauth_device_flow_ids
                && self.oauth_device_flow_expires_at_millis.keys() == self.oauth_device_flow_ids
                && for_all(flow_id in self.oauth_device_poll_ids, self.oauth_device_flow_ids.contains(flow_id))
                && self.oauth_outstanding_flow_count == self.oauth_browser_flow_ids.len() + self.oauth_device_flow_ids.len()
            }

            // --- Transitions ---

            transition Acquire {
                on input Acquire { expires_at_ts, credential_published_at_millis }
                update {
                    self.expires_at = expires_at_ts;
                    self.refresh_attempt = 0;
                    self.credential_present = true;
                    self.credential_generation = self.credential_generation + 1;
                    self.credential_published_at_millis = Some(credential_published_at_millis);
                }
                to Valid
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition MarkExpiring {
                on input MarkExpiring
                guard { self.lifecycle_phase == Phase::Valid }
                to Expiring
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition BeginRefreshFromValid {
                on input BeginRefresh
                guard { self.lifecycle_phase == Phase::Valid }
                to Refreshing
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
                emit WakeRefreshLoop
            }

            transition BeginRefreshFromExpiring {
                on input BeginRefresh
                guard { self.lifecycle_phase == Phase::Expiring }
                to Refreshing
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
                emit WakeRefreshLoop
            }

            transition CompleteRefresh {
                on input CompleteRefresh { new_expires_at, now_ts, credential_published_at_millis }
                guard { self.lifecycle_phase == Phase::Refreshing }
                update {
                    self.expires_at = new_expires_at;
                    self.last_refresh = Some(now_ts);
                    self.refresh_attempt = 0;
                    self.credential_present = true;
                    self.credential_generation = self.credential_generation + 1;
                    self.credential_published_at_millis = Some(credential_published_at_millis);
                }
                to Valid
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RefreshFailedTransient {
                on input RefreshFailedTransient
                guard { self.lifecycle_phase == Phase::Refreshing }
                update {
                    self.refresh_attempt = self.refresh_attempt + 1;
                }
                to Expiring
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RefreshFailedPermanent {
                on input RefreshFailedPermanent
                guard { self.lifecycle_phase == Phase::Refreshing }
                update {
                    self.refresh_attempt = self.refresh_attempt + 1;
                }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition MarkReauthRequiredFromValid {
                on input MarkReauthRequired
                guard { self.lifecycle_phase == Phase::Valid }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition MarkReauthRequiredFromExpiring {
                on input MarkReauthRequired
                guard { self.lifecycle_phase == Phase::Expiring }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition MarkReauthRequiredFromRefreshing {
                on input MarkReauthRequired
                guard { self.lifecycle_phase == Phase::Refreshing }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ClearCredentialLifecycle {
                on input ClearCredentialLifecycle
                update {
                    self.expires_at = None;
                    self.last_refresh = None;
                    self.refresh_attempt = 0;
                    self.credential_present = false;
                    self.credential_published_at_millis = None;
                }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition Release {
                on input Release
                update {
                    self.expires_at = None;
                    self.last_refresh = None;
                    self.refresh_attempt = 0;
                    self.credential_present = false;
                    self.credential_published_at_millis = None;
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RestoreAuthoritySnapshotValid {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis
                }
                guard { lifecycle_phase == Phase::Valid && credential_present && credential_published_at_millis != None }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    if credential_generation > self.credential_generation {
                        self.credential_generation = credential_generation;
                    }
                    self.credential_published_at_millis = credential_published_at_millis;
                }
                to Valid
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RestoreAuthoritySnapshotExpiring {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis
                }
                guard { lifecycle_phase == Phase::Expiring && credential_present && credential_published_at_millis != None }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    if credential_generation > self.credential_generation {
                        self.credential_generation = credential_generation;
                    }
                    self.credential_published_at_millis = credential_published_at_millis;
                }
                to Expiring
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RestoreAuthoritySnapshotRefreshing {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis
                }
                guard { lifecycle_phase == Phase::Refreshing && credential_present && credential_published_at_millis != None }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    if credential_generation > self.credential_generation {
                        self.credential_generation = credential_generation;
                    }
                    self.credential_published_at_millis = credential_published_at_millis;
                }
                to Refreshing
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RestoreAuthoritySnapshotReauthRequired {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis
                }
                guard { lifecycle_phase == Phase::ReauthRequired && (credential_present == false || credential_published_at_millis != None) }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    if credential_generation > self.credential_generation {
                        self.credential_generation = credential_generation;
                    }
                    self.credential_published_at_millis = credential_published_at_millis;
                }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RestoreAuthoritySnapshotReleased {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis
                }
                guard { lifecycle_phase == Phase::Released && credential_present == false && credential_published_at_millis == None && self.oauth_outstanding_flow_count == 0 }
                update {
                    self.expires_at = expires_at;
                    self.last_refresh = last_refresh;
                    self.refresh_attempt = refresh_attempt;
                    self.credential_present = credential_present;
                    if credential_generation > self.credential_generation {
                        self.credential_generation = credential_generation;
                    }
                    self.credential_published_at_millis = credential_published_at_millis;
                }
                to Released
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RestoreOAuthBrowserFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input RestoreOAuthBrowserFlow { flow_id, provider, redirect_uri, expires_at_millis }
                update {
                    if self.oauth_browser_flow_ids.contains(flow_id) == false {
                        self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count + 1;
                    }
                    self.oauth_browser_flow_ids.insert(flow_id);
                    self.oauth_browser_flow_providers.insert(flow_id, provider);
                    self.oauth_browser_flow_redirect_uris.insert(flow_id, redirect_uri);
                    self.oauth_browser_flow_expires_at_millis.insert(flow_id, expires_at_millis);
                }
                to Valid
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RestoreOAuthDeviceFlow {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input RestoreOAuthDeviceFlow { flow_id, provider, expires_at_millis, poll_active }
                update {
                    if self.oauth_device_flow_ids.contains(flow_id) == false {
                        self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count + 1;
                    }
                    self.oauth_device_flow_ids.insert(flow_id);
                    self.oauth_device_flow_providers.insert(flow_id, provider);
                    self.oauth_device_flow_expires_at_millis.insert(flow_id, expires_at_millis);
                    if poll_active {
                        self.oauth_device_poll_ids.insert(flow_id);
                    } else {
                        self.oauth_device_poll_ids.remove(flow_id);
                    }
                }
                to Valid
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ReopenReleasedForOAuthBrowserFlowAdmission {
                on input AdmitOAuthBrowserFlow { flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows }
                guard { self.lifecycle_phase == Phase::Released }
                guard "released_without_credential" { self.credential_present == false && self.credential_published_at_millis == None }
                guard "released_without_oauth_membership" { self.oauth_outstanding_flow_count == 0 }
                guard "oauth_capacity_available" { self.oauth_outstanding_flow_count < max_outstanding_flows }
                guard "oauth_global_capacity_available" { observed_global_outstanding_flows < max_outstanding_flows }
                update {
                    self.oauth_browser_flow_ids.insert(flow_id);
                    self.oauth_browser_flow_providers.insert(flow_id, provider);
                    self.oauth_browser_flow_redirect_uris.insert(flow_id, redirect_uri);
                    self.oauth_browser_flow_expires_at_millis.insert(flow_id, expires_at_millis);
                    self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count + 1;
                }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ReopenReleasedForOAuthDeviceFlowAdmission {
                on input AdmitOAuthDeviceFlow { flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows }
                guard { self.lifecycle_phase == Phase::Released }
                guard "released_without_credential" { self.credential_present == false && self.credential_published_at_millis == None }
                guard "released_without_oauth_membership" { self.oauth_outstanding_flow_count == 0 }
                guard "oauth_capacity_available" { self.oauth_outstanding_flow_count < max_outstanding_flows }
                guard "oauth_global_capacity_available" { observed_global_outstanding_flows < max_outstanding_flows }
                update {
                    self.oauth_device_flow_ids.insert(flow_id);
                    self.oauth_device_flow_providers.insert(flow_id, provider);
                    self.oauth_device_flow_expires_at_millis.insert(flow_id, expires_at_millis);
                    self.oauth_device_poll_ids.remove(flow_id);
                    self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count + 1;
                }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition FinishOAuthDevicePoll {
                per_phase [Valid, Expiring, Refreshing, ReauthRequired]
                on input FinishOAuthDevicePoll { flow_id }
                guard "device_poll_present" { self.oauth_device_poll_ids.contains(flow_id) }
                update {
                    self.oauth_device_poll_ids.remove(flow_id);
                }
                to Valid
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
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
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }
        }
            }
    };
}

crate::auth_catalog_machine_dsl!("self", "catalog::dsl::auth_machine");
