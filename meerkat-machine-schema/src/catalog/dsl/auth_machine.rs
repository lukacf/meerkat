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
        /// Typed credential-use intent fed by the resolver shell. Identifies
        /// WHICH credential gate is asking, never a policy decision: the
        /// AuthMachine owns the (phase, credential_present, intent) ->
        /// disposition verdict.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum CredentialUseIntent {
            /// Resolver "use the credential now" read (`resolve_managed_store_secret`).
            UseCredential,
            /// Resolver post-publish lifecycle-authority gate
            /// (`require_credential_lifecycle_authority`).
            HoldAuthority,
            /// Resolver OAuth-refresh begin gate
            /// (`begin_managed_store_oauth_refresh_lifecycle`).
            BeginRefresh,
        }

        /// Machine-owned credential-use disposition verdict the resolver shell
        /// mirrors rather than deciding.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum CredentialUseDisposition {
            /// Credential may be used / lifecycle authority is held.
            Authorized,
            /// Caller must refresh first (BeginRefresh callers fire `begin_refresh`).
            RefreshRequired,
            /// A refresh is required to proceed but the binding's config does not
            /// permit silent refresh (`allow_refresh == false`); the caller
            /// surfaces a refresh-required error instead of beginning a refresh.
            RefreshDisallowed,
            /// Interactive user reauthorization is required.
            ReauthRequired,
            /// No usable lease is present.
            LeaseAbsent,
            /// A refresh is already in flight; the begin-refresh caller no-ops.
            AlreadyRefreshing,
        }

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
                // Release-drain sub-state (0.7.2 D1): true between BeginRelease
                // and the final Release commit. While draining, new OAuth flow
                // admissions are refused so the drain obligation set cannot
                // grow behind the shell's back; the Release transition is
                // guarded on the drain being complete.
                release_draining: bool,
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
                release_draining = false,
            }

            terminal [Released]

            phase AuthLifecyclePhase {
                Valid,
                Expiring,
                Expired,
                Refreshing,
                ReauthRequired,
                Released,
            }

            input AuthMachineInput {
                Acquire { expires_at_ts: Option<u64>, credential_published_at_millis: u64 },
                MarkExpiring,
                ObserveCredentialFreshness { now_ts: u64, refresh_window_secs: u64 },
                BeginRefresh,
                CompleteRefresh { new_expires_at: Option<u64>, now_ts: u64, credential_published_at_millis: u64 },
                RefreshFailed {
                    http_status: Option<u64>,
                    oauth_error_code: Option<String>,
                    local_credential_unusable: bool,
                },
                MarkReauthRequired,
                ClearCredentialLifecycle,
                ReleaseCredentialLifecycle,
                // Release teardown is two-phase (0.7.2 D1). BeginRelease records
                // the draining intent and emits the in-flight OAuth flow
                // membership as a typed cancellation obligation
                // (`CancelOAuthFlowsForRelease`). The shell discharges it by
                // terminally cancelling each flow (ExpireOAuthBrowserFlow /
                // ExpireOAuthDeviceFlow feedback inputs) and pruning the
                // registry payloads; only then is Release legal.
                BeginRelease,
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
                RestoreCredentialLifecycleSnapshot {
                    lifecycle_phase: Option<Enum<AuthLifecyclePhase>>,
                    expires_at: Option<u64>,
                    last_refresh: Option<u64>,
                    refresh_attempt: u64,
                    credential_present: bool,
                    credential_generation: u64,
                    credential_published_at_millis: Option<u64>,
                    restored_oauth_membership_observed: bool,
                },
                RestoreOAuthBrowserFlow { flow_id: String, provider: Option<String>, redirect_uri: Option<String>, expires_at_millis: Option<u64> },
                RestoreOAuthDeviceFlow { flow_id: String, provider: Option<String>, expires_at_millis: Option<u64> },
                RestoreOAuthDevicePoll { flow_id: String },
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
                // Credential-use admission classification. The resolver shell
                // (meerkat-auth-core) extracts only the typed `intent` — WHICH
                // credential gate is asking (use the credential now / hold
                // post-publish lifecycle authority / begin an OAuth refresh) —
                // and drives this read-only input. This machine owns the
                // COMPLETE (lifecycle_phase, credential_present, intent) ->
                // disposition POLICY using its own state, emitting
                // `CredentialUseAdmissionResolved`. The shell mirrors the
                // verdict and fails closed; it decides nothing. Each transition
                // self-loops in its phase (classification never mutates state).
                ResolveCredentialUseAdmission { intent: Enum<CredentialUseIntent> },
                // OAuth-login cached-vs-refresh disposition. The provider
                // runtime shell (meerkat-{anthropic,openai,gemini}) extracts only
                // the pure observations it holds — whether a persisted credential
                // secret is present (`credential_present`), whether the caller
                // forced a refresh (`force_refresh`), and whether the binding
                // config permits silent refresh (`refresh_allowed`) — and drives
                // this read-only input. This machine composes the COMPLETE
                // (lifecycle_phase, self.credential_present, credential_present,
                // force_refresh, refresh_allowed) -> disposition policy, emitting
                // `CredentialUseAdmissionResolved`: Authorized -> use the cached
                // credential, RefreshRequired -> begin an OAuth refresh,
                // RefreshDisallowed -> refresh-required error, ReauthRequired ->
                // reauth error, LeaseAbsent -> lease-absent error. The shell
                // mirrors the verdict and decides nothing. Each transition
                // self-loops in its phase (classification never mutates state).
                ResolveOAuthLoginCredentialDisposition {
                    credential_present: bool,
                    force_refresh: bool,
                    refresh_allowed: bool,
                },
            }

            effect AuthMachineEffect {
                EmitLifecycleEvent {
                    new_state: AuthLifecyclePhase,
                    expires_at: Option<u64>,
                    credential_generation: u64,
                    credential_published_at_millis: Option<u64>,
                },
                WakeRefreshLoop,
                // Credential-use disposition decided by this machine. The
                // resolver shell mirrors `disposition`: Authorized -> use the
                // credential / proceed, RefreshRequired -> the caller refreshes
                // (BeginRefresh callers fire `begin_refresh`), ReauthRequired ->
                // user-reauth error, LeaseAbsent -> lease-absent error,
                // AlreadyRefreshing -> a refresh is already in flight (no-op).
                CredentialUseAdmissionResolved { disposition: Enum<CredentialUseDisposition> },
                // Typed terminal-cancellation obligation for release teardown
                // (0.7.2 D1). Emitted by BeginRelease while the flow membership
                // is still intact: it carries exactly the in-flight browser and
                // device flows the shell must terminally cancel (Expire*
                // feedback inputs + registry payload pruning) before the final
                // Release transition becomes legal. Machine-owned replacement
                // for the shell's prior direct read of flow membership during
                // release staging.
                CancelOAuthFlowsForRelease {
                    browser_flow_ids: Set<String>,
                    device_flow_ids: Set<String>,
                },
            }

            disposition EmitLifecycleEvent => external handoff auth_lease_lifecycle_publication seam SurfaceResultAlignment,
            disposition WakeRefreshLoop => local seam NoOwnerRealization,
            disposition CredentialUseAdmissionResolved => local seam SurfaceResultAlignment,
            disposition CancelOAuthFlowsForRelease => external handoff auth_release_oauth_flow_drain seam NoOwnerRealization,

            invariant oauth_flow_membership_consistent {
                self.oauth_browser_flow_providers.keys() == self.oauth_browser_flow_ids
                && self.oauth_browser_flow_redirect_uris.keys() == self.oauth_browser_flow_ids
                && self.oauth_browser_flow_expires_at_millis.keys() == self.oauth_browser_flow_ids
                && self.oauth_device_flow_providers.keys() == self.oauth_device_flow_ids
                && self.oauth_device_flow_expires_at_millis.keys() == self.oauth_device_flow_ids
                && for_all(flow_id in self.oauth_device_poll_ids, self.oauth_device_flow_ids.contains(flow_id))
                && self.oauth_outstanding_flow_count == self.oauth_browser_flow_ids.len() + self.oauth_device_flow_ids.len()
            }

            // Release-drain soundness (0.7.2 D1): a Released machine can never
            // hold OAuth flow membership — every in-flight flow received a
            // typed terminal cancellation before Release committed — so no
            // producer can hold a flow fact that later "expires" into a
            // Released machine.
            invariant released_oauth_membership_drained {
                self.lifecycle_phase != Phase::Released || self.oauth_outstanding_flow_count == 0
            }

            invariant released_not_release_draining {
                self.lifecycle_phase != Phase::Released || self.release_draining == false
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

            transition ObserveCredentialFreshnessValid {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::Valid }
                guard "credential_not_in_refresh_window" {
                    if self.expires_at == None {
                        true
                    } else {
                        now_ts + refresh_window_secs <= self.expires_at.get("value")
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

            transition ObserveCredentialFreshnessExpiringFromValid {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::Valid }
                guard "credential_in_refresh_window" {
                    if self.expires_at == None {
                        false
                    } else {
                        now_ts < self.expires_at.get("value")
                        && self.expires_at.get("value") < now_ts + refresh_window_secs
                    }
                }
                to Expiring
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ObserveCredentialFreshnessExpiredFromValid {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::Valid }
                guard "credential_expired" {
                    if self.expires_at == None {
                        false
                    } else {
                        self.expires_at.get("value") <= now_ts
                    }
                }
                to Expired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ObserveCredentialFreshnessExpiring {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::Expiring }
                guard "credential_not_expired" {
                    if self.expires_at == None {
                        true
                    } else {
                        now_ts < self.expires_at.get("value")
                    }
                }
                to Expiring
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ObserveCredentialFreshnessExpiredFromExpiring {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::Expiring }
                guard "credential_expired" {
                    if self.expires_at == None {
                        false
                    } else {
                        self.expires_at.get("value") <= now_ts
                    }
                }
                to Expired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ObserveCredentialFreshnessExpired {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::Expired }
                to Expired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ObserveCredentialFreshnessRefreshing {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::Refreshing }
                to Refreshing
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ObserveCredentialFreshnessReauthRequired {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::ReauthRequired }
                to ReauthRequired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition ObserveCredentialFreshnessReleased {
                on input ObserveCredentialFreshness { now_ts, refresh_window_secs }
                guard { self.lifecycle_phase == Phase::Released }
                to Released
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

            transition BeginRefreshFromExpired {
                on input BeginRefresh
                guard { self.lifecycle_phase == Phase::Expired }
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
                guard "new_credential_not_expired" {
                    if new_expires_at == None {
                        true
                    } else {
                        now_ts < new_expires_at.get("value")
                    }
                }
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
                on input RefreshFailed { http_status, oauth_error_code, local_credential_unusable }
                guard { self.lifecycle_phase == Phase::Refreshing }
                guard "refresh_failure_observation_transient" {
                    local_credential_unusable == false
                    && http_status != Some(401)
                    && http_status != Some(403)
                    && oauth_error_code != Some("invalid_grant")
                    && oauth_error_code != Some("invalid_client")
                    && oauth_error_code != Some("unauthorized_client")
                    && oauth_error_code != Some("invalid_scope")
                    && oauth_error_code != Some("access_denied")
                    && oauth_error_code != Some("permission_denied")
                    && oauth_error_code != Some("expired_token")
                }
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
                on input RefreshFailed { http_status, oauth_error_code, local_credential_unusable }
                guard { self.lifecycle_phase == Phase::Refreshing }
                guard "refresh_failure_observation_permanent" {
                    local_credential_unusable == true
                    || http_status == Some(401)
                    || http_status == Some(403)
                    || oauth_error_code == Some("invalid_grant")
                    || oauth_error_code == Some("invalid_client")
                    || oauth_error_code == Some("unauthorized_client")
                    || oauth_error_code == Some("invalid_scope")
                    || oauth_error_code == Some("access_denied")
                    || oauth_error_code == Some("permission_denied")
                    || oauth_error_code == Some("expired_token")
                }
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

            transition MarkReauthRequiredFromExpired {
                on input MarkReauthRequired
                guard { self.lifecycle_phase == Phase::Expired }
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

            transition ReleaseCredentialLifecycleWithOAuth {
                on input ReleaseCredentialLifecycle
                guard "oauth_membership_present" { self.oauth_outstanding_flow_count > 0 }
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

            transition ReleaseCredentialLifecycleWithoutOAuth {
                on input ReleaseCredentialLifecycle
                guard "oauth_membership_absent" { self.oauth_outstanding_flow_count == 0 }
                update {
                    self.release_draining = false;
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

            // --- Release drain (0.7.2 D1) ---
            //
            // release_lease teardown is two-phase and machine-owned. BeginRelease
            // records the draining intent (`release_draining`) and, when OAuth
            // flows are in flight, emits `CancelOAuthFlowsForRelease` carrying
            // the exact membership to terminally cancel. The shell discharges
            // the obligation with ExpireOAuthBrowserFlow / ExpireOAuthDeviceFlow
            // feedback inputs (each one removes the flow and decrements the
            // outstanding count) plus registry payload pruning. The final
            // Release transition is guarded on the drain being complete, so a
            // Released machine structurally cannot leave flows behind that a
            // late prune/compensation producer would try to expire into it.
            transition BeginReleaseDrainingOAuthFlows {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input BeginRelease
                guard "oauth_membership_present" { self.oauth_outstanding_flow_count > 0 }
                update {
                    self.release_draining = true;
                }
                to Valid
                emit CancelOAuthFlowsForRelease {
                    browser_flow_ids: self.oauth_browser_flow_ids,
                    device_flow_ids: self.oauth_device_flow_ids,
                }
            }

            transition BeginReleaseWithoutOAuthFlows {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input BeginRelease
                guard "oauth_membership_absent" { self.oauth_outstanding_flow_count == 0 }
                update {
                    self.release_draining = true;
                }
                to Valid
            }

            // D2a: beginning release of an already-released lease is a
            // legitimate arrival (release retry / unregister racing release);
            // accepted as a no-op, never a guard rejection.
            transition BeginReleaseReleased {
                on input BeginRelease
                guard { self.lifecycle_phase == Phase::Released }
                update {}
                to Released
            }

            transition Release {
                on input Release
                guard "oauth_release_drained" { self.oauth_outstanding_flow_count == 0 }
                update {
                    self.release_draining = false;
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

            transition RestoreCredentialLifecycleSnapshotValid {
                on input RestoreCredentialLifecycleSnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis,
                    restored_oauth_membership_observed
                }
                guard { lifecycle_phase == Some(AuthLifecyclePhase::Valid) && credential_present && credential_published_at_millis != None }
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

            transition RestoreCredentialLifecycleSnapshotExpiring {
                on input RestoreCredentialLifecycleSnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis,
                    restored_oauth_membership_observed
                }
                guard { lifecycle_phase == Some(AuthLifecyclePhase::Expiring) && credential_present && credential_published_at_millis != None }
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

            transition RestoreCredentialLifecycleSnapshotRefreshing {
                on input RestoreCredentialLifecycleSnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis,
                    restored_oauth_membership_observed
                }
                guard { lifecycle_phase == Some(AuthLifecyclePhase::Refreshing) && credential_present && credential_published_at_millis != None }
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

            transition RestoreCredentialLifecycleSnapshotExpired {
                on input RestoreCredentialLifecycleSnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis,
                    restored_oauth_membership_observed
                }
                guard { lifecycle_phase == Some(AuthLifecyclePhase::Expired) && credential_present && credential_published_at_millis != None }
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
                to Expired
                emit EmitLifecycleEvent {
                    new_state: self.lifecycle_phase,
                    expires_at: self.expires_at,
                    credential_generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                }
            }

            transition RestoreCredentialLifecycleSnapshotReauthRequired {
                on input RestoreCredentialLifecycleSnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis,
                    restored_oauth_membership_observed
                }
                guard { lifecycle_phase == Some(AuthLifecyclePhase::ReauthRequired) && credential_present && credential_published_at_millis != None }
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

            transition RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth {
                on input RestoreCredentialLifecycleSnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis,
                    restored_oauth_membership_observed
                }
                guard "restore_snapshot_has_no_credential" { credential_present == false || lifecycle_phase == None || lifecycle_phase == Some(AuthLifecyclePhase::Released) }
                guard "restore_oauth_membership_present" { self.oauth_outstanding_flow_count > 0 || restored_oauth_membership_observed }
                update {
                    self.expires_at = None;
                    self.last_refresh = None;
                    self.refresh_attempt = 0;
                    self.credential_present = false;
                    if credential_generation > self.credential_generation {
                        self.credential_generation = credential_generation;
                    }
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

            transition RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth {
                on input RestoreCredentialLifecycleSnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis,
                    restored_oauth_membership_observed
                }
                guard "restore_snapshot_has_no_credential" { credential_present == false || lifecycle_phase == None || lifecycle_phase == Some(AuthLifecyclePhase::Released) }
                guard "restore_oauth_membership_absent" { self.oauth_outstanding_flow_count == 0 && restored_oauth_membership_observed == false }
                update {
                    self.release_draining = false;
                    self.expires_at = None;
                    self.last_refresh = None;
                    self.refresh_attempt = 0;
                    self.credential_present = false;
                    if credential_generation > self.credential_generation {
                        self.credential_generation = credential_generation;
                    }
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

            transition RestoreAuthoritySnapshotExpired {
                on input RestoreAuthoritySnapshot {
                    lifecycle_phase,
                    expires_at,
                    last_refresh,
                    refresh_attempt,
                    credential_present,
                    credential_generation,
                    credential_published_at_millis
                }
                guard { lifecycle_phase == Phase::Expired && credential_present && credential_published_at_millis != None }
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
                to Expired
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
                    self.release_draining = false;
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input RestoreOAuthBrowserFlow { flow_id, provider, redirect_uri, expires_at_millis }
                guard "not_release_draining" { self.release_draining == false }
                guard "browser_restore_has_provider" { provider != None }
                guard "browser_restore_has_redirect_uri" { redirect_uri != None }
                guard "browser_restore_has_expiry" { expires_at_millis != None }
                update {
                    if self.oauth_browser_flow_ids.contains(flow_id) == false {
                        self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count + 1;
                    }
                    self.oauth_browser_flow_ids.insert(flow_id);
                    self.oauth_browser_flow_providers.insert(flow_id, provider.get("value"));
                    self.oauth_browser_flow_redirect_uris.insert(flow_id, redirect_uri.get("value"));
                    self.oauth_browser_flow_expires_at_millis.insert(flow_id, expires_at_millis.get("value"));
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input RestoreOAuthDeviceFlow { flow_id, provider, expires_at_millis }
                guard "not_release_draining" { self.release_draining == false }
                guard "device_restore_has_provider" { provider != None }
                guard "device_restore_has_expiry" { expires_at_millis != None }
                update {
                    if self.oauth_device_flow_ids.contains(flow_id) == false {
                        self.oauth_outstanding_flow_count = self.oauth_outstanding_flow_count + 1;
                    }
                    self.oauth_device_flow_ids.insert(flow_id);
                    self.oauth_device_flow_providers.insert(flow_id, provider.get("value"));
                    self.oauth_device_flow_expires_at_millis.insert(flow_id, expires_at_millis.get("value"));
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

            transition RestoreOAuthDevicePoll {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input RestoreOAuthDevicePoll { flow_id }
                guard "not_release_draining" { self.release_draining == false }
                guard "device_flow_present_for_poll_restore" { self.oauth_device_flow_ids.contains(flow_id) }
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

            // `per_phase` expands each OAuth flow transition into one
            // transition per listed phase and rewrites the target to that same
            // phase. The syntactic `to Valid` below is therefore only a grammar
            // placeholder; OAuth membership changes must not manufacture a
            // credential-valid lease.
            transition AdmitOAuthBrowserFlow {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input AdmitOAuthBrowserFlow { flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows }
                guard "not_release_draining" { self.release_draining == false }
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
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

            // D2a (0.7.2): expiry is observation-shaped ("this flow is dead"),
            // delivered by poll/prune producers and error-handler compensation
            // that legitimately race flow consumption and lease release. A
            // stale expiry for a flow that is no longer a member — or that
            // arrives after the lease was Released — is a benign no-op, never
            // a guard rejection (worklist entries 24, 25, 27, 30).
            transition ExpireOAuthBrowserFlowAbsent {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input ExpireOAuthBrowserFlow { flow_id }
                guard "browser_flow_absent" { self.oauth_browser_flow_ids.contains(flow_id) == false }
                update {}
                to Valid
            }

            transition ExpireOAuthBrowserFlowReleased {
                on input ExpireOAuthBrowserFlow { flow_id }
                guard { self.lifecycle_phase == Phase::Released }
                update {}
                to Released
            }

            transition AdmitOAuthDeviceFlow {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input AdmitOAuthDeviceFlow { flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows }
                guard "not_release_draining" { self.release_draining == false }
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input ConfirmOAuthDurableAdmission { observed_global_outstanding_flows, max_outstanding_flows }
                guard "oauth_global_capacity_available" { observed_global_outstanding_flows < max_outstanding_flows }
                update {}
                to Valid
            }

            // D2a (0.7.2): the durable-admission confirmation fires from
            // inside the store persistence callback and can legitimately
            // arrive after a concurrent release_lease committed Release. The
            // admitted flow was already terminally cancelled by the release
            // drain, so the confirmation is a benign no-op — the previous
            // guard rejection failed the whole persistence path for a
            // committed teardown interleave (worklist entry 29). In live
            // phases the capacity guard above remains the authoritative
            // fail-closed admission check.
            transition ConfirmOAuthDurableAdmissionReleased {
                on input ConfirmOAuthDurableAdmission { observed_global_outstanding_flows, max_outstanding_flows }
                guard { self.lifecycle_phase == Phase::Released }
                update {}
                to Released
            }

            transition VerifyOAuthDeviceFlow {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
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

            // D2a (0.7.2): poll-finish is cleanup-shaped (poll-lease drop
            // guards and error-handler compensation fire it) and shares the
            // exact race shape of the Expire* inputs — the poll membership can
            // have been removed by a concurrent consume/expire/release before
            // the finish lands. Sibling-completeness of the worklist's
            // Expire*/Confirm* totality.
            transition FinishOAuthDevicePollAbsent {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input FinishOAuthDevicePoll { flow_id }
                guard "device_poll_absent" { self.oauth_device_poll_ids.contains(flow_id) == false }
                update {}
                to Valid
            }

            transition FinishOAuthDevicePollReleased {
                on input FinishOAuthDevicePoll { flow_id }
                guard { self.lifecycle_phase == Phase::Released }
                update {}
                to Released
            }

            transition ConsumeOAuthDeviceFlow {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
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
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
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

            // D2a (0.7.2): same totality as browser-flow expiry — stale or
            // post-release device-flow expiry observations are benign no-ops
            // (worklist entries 26, 28, 30).
            transition ExpireOAuthDeviceFlowAbsent {
                per_phase [Valid, Expiring, Expired, Refreshing, ReauthRequired]
                on input ExpireOAuthDeviceFlow { flow_id }
                guard "device_flow_absent" { self.oauth_device_flow_ids.contains(flow_id) == false }
                update {}
                to Valid
            }

            transition ExpireOAuthDeviceFlowReleased {
                on input ExpireOAuthDeviceFlow { flow_id }
                guard { self.lifecycle_phase == Phase::Released }
                update {}
                to Released
            }

            // --- Credential-use admission classification ---
            //
            // This machine owns the COMPLETE credential-use disposition POLICY,
            // reproducing exactly the behavior the resolver shell previously
            // duplicated across four `match phase` reducers. The verdict is a
            // pure function of this machine's own `lifecycle_phase` and
            // `credential_present`, plus the typed `intent` the shell feeds:
            //
            //   UseCredential  (resolver "use the credential now" read):
            //     Valid+cred                -> Authorized
            //     {Expiring,Expired,Refreshing}+cred -> RefreshRequired
            //     ReauthRequired            -> ReauthRequired
            //     otherwise                 -> LeaseAbsent
            //   HoldAuthority  (resolver post-publish lifecycle-authority gate):
            //     {Valid,Expiring,Refreshing}+cred -> Authorized
            //     Expired+cred              -> RefreshRequired
            //     ReauthRequired            -> ReauthRequired
            //     otherwise                 -> LeaseAbsent
            //   BeginRefresh   (resolver OAuth-refresh begin gate):
            //     {Valid,Expiring,Expired}+cred -> RefreshRequired
            //     Refreshing                -> AlreadyRefreshing
            //     ReauthRequired            -> ReauthRequired
            //     otherwise                 -> LeaseAbsent
            //
            // Each transition self-loops in its phase. The (phase, intent,
            // credential_present) coverage is total and mutually exclusive.

            // Valid
            transition ResolveCredentialUseAdmissionValidUseAuthorized {
                per_phase [Valid]
                on input ResolveCredentialUseAdmission { intent }
                guard "valid_use_authorized" {
                    intent == CredentialUseIntent::UseCredential && self.credential_present
                }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::Authorized }
            }
            transition ResolveCredentialUseAdmissionValidHoldAuthorized {
                per_phase [Valid]
                on input ResolveCredentialUseAdmission { intent }
                guard "valid_hold_authorized" {
                    intent == CredentialUseIntent::HoldAuthority && self.credential_present
                }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::Authorized }
            }
            transition ResolveCredentialUseAdmissionValidBeginRefresh {
                per_phase [Valid]
                on input ResolveCredentialUseAdmission { intent }
                guard "valid_begin_refresh" {
                    intent == CredentialUseIntent::BeginRefresh && self.credential_present
                }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }
            transition ResolveCredentialUseAdmissionValidNoCredential {
                per_phase [Valid]
                on input ResolveCredentialUseAdmission { intent }
                guard "valid_no_credential" { self.credential_present == false }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::LeaseAbsent }
            }

            // Expiring
            transition ResolveCredentialUseAdmissionExpiringUseRefresh {
                per_phase [Expiring]
                on input ResolveCredentialUseAdmission { intent }
                guard "expiring_use_refresh" {
                    intent == CredentialUseIntent::UseCredential && self.credential_present
                }
                update {}
                to Expiring
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }
            transition ResolveCredentialUseAdmissionExpiringHoldAuthorized {
                per_phase [Expiring]
                on input ResolveCredentialUseAdmission { intent }
                guard "expiring_hold_authorized" {
                    intent == CredentialUseIntent::HoldAuthority && self.credential_present
                }
                update {}
                to Expiring
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::Authorized }
            }
            transition ResolveCredentialUseAdmissionExpiringBeginRefresh {
                per_phase [Expiring]
                on input ResolveCredentialUseAdmission { intent }
                guard "expiring_begin_refresh" {
                    intent == CredentialUseIntent::BeginRefresh && self.credential_present
                }
                update {}
                to Expiring
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }
            transition ResolveCredentialUseAdmissionExpiringNoCredential {
                per_phase [Expiring]
                on input ResolveCredentialUseAdmission { intent }
                guard "expiring_no_credential" { self.credential_present == false }
                update {}
                to Expiring
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::LeaseAbsent }
            }

            // Expired
            transition ResolveCredentialUseAdmissionExpiredUseRefresh {
                per_phase [Expired]
                on input ResolveCredentialUseAdmission { intent }
                guard "expired_use_refresh" {
                    intent == CredentialUseIntent::UseCredential && self.credential_present
                }
                update {}
                to Expired
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }
            transition ResolveCredentialUseAdmissionExpiredHoldRefresh {
                per_phase [Expired]
                on input ResolveCredentialUseAdmission { intent }
                guard "expired_hold_refresh" {
                    intent == CredentialUseIntent::HoldAuthority && self.credential_present
                }
                update {}
                to Expired
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }
            transition ResolveCredentialUseAdmissionExpiredBeginRefresh {
                per_phase [Expired]
                on input ResolveCredentialUseAdmission { intent }
                guard "expired_begin_refresh" {
                    intent == CredentialUseIntent::BeginRefresh && self.credential_present
                }
                update {}
                to Expired
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }
            transition ResolveCredentialUseAdmissionExpiredNoCredential {
                per_phase [Expired]
                on input ResolveCredentialUseAdmission { intent }
                guard "expired_no_credential" { self.credential_present == false }
                update {}
                to Expired
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::LeaseAbsent }
            }

            // Refreshing
            transition ResolveCredentialUseAdmissionRefreshingUseRefresh {
                per_phase [Refreshing]
                on input ResolveCredentialUseAdmission { intent }
                guard "refreshing_use_refresh" {
                    intent == CredentialUseIntent::UseCredential && self.credential_present
                }
                update {}
                to Refreshing
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }
            transition ResolveCredentialUseAdmissionRefreshingHoldAuthorized {
                per_phase [Refreshing]
                on input ResolveCredentialUseAdmission { intent }
                guard "refreshing_hold_authorized" {
                    intent == CredentialUseIntent::HoldAuthority && self.credential_present
                }
                update {}
                to Refreshing
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::Authorized }
            }
            transition ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshing {
                per_phase [Refreshing]
                on input ResolveCredentialUseAdmission { intent }
                guard "refreshing_begin_already" {
                    intent == CredentialUseIntent::BeginRefresh
                }
                update {}
                to Refreshing
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::AlreadyRefreshing }
            }
            transition ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHold {
                per_phase [Refreshing]
                on input ResolveCredentialUseAdmission { intent }
                guard "refreshing_no_credential" {
                    self.credential_present == false
                    && (
                        intent == CredentialUseIntent::UseCredential
                        || intent == CredentialUseIntent::HoldAuthority
                    )
                }
                update {}
                to Refreshing
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::LeaseAbsent }
            }

            // ReauthRequired — credential state irrelevant.
            transition ResolveCredentialUseAdmissionReauthRequired {
                per_phase [ReauthRequired]
                on input ResolveCredentialUseAdmission { intent }
                update {}
                to ReauthRequired
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::ReauthRequired }
            }

            // Released — always lease-absent.
            transition ResolveCredentialUseAdmissionReleased {
                per_phase [Released]
                on input ResolveCredentialUseAdmission { intent }
                update {}
                to Released
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::LeaseAbsent }
            }

            // --- OAuth-login cached-vs-refresh disposition ---
            //
            // Composes the provider-runtime shell's prior composite
            // (`lifecycle == Authorized && primary_secret.is_some() &&
            // !force_refresh` use-cached; else `refresh_allowed ? begin-refresh :
            // Err(RefreshRequired)`). `lifecycle == Authorized` is exactly this
            // machine's `UseCredential` verdict — phase Valid with its own
            // `self.credential_present`. The shell's `primary_secret.is_some()`
            // arrives as the `credential_present` observation, the force-refresh
            // override as `force_refresh`, and the binding refresh-capability
            // config as `refresh_allowed`. Each transition self-loops; the
            // guards partition the full input space.

            // Use the cached credential: phase Valid with a published credential,
            // a present persisted secret, and no forced refresh.
            transition ResolveOAuthLoginCredentialDispositionUseCached {
                per_phase [Valid]
                on input ResolveOAuthLoginCredentialDisposition { credential_present, force_refresh, refresh_allowed }
                guard "use_cached" {
                    self.credential_present
                    && credential_present
                    && force_refresh == false
                }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::Authorized }
            }

            // Begin a refresh: not eligible to use the cached credential and the
            // binding permits silent refresh.
            transition ResolveOAuthLoginCredentialDispositionRefreshValid {
                per_phase [Valid]
                on input ResolveOAuthLoginCredentialDisposition { credential_present, force_refresh, refresh_allowed }
                guard "needs_refresh_allowed" {
                    !(self.credential_present && credential_present && force_refresh == false)
                    && refresh_allowed
                }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }

            // Refresh required but config-disallowed -> refresh-required error.
            transition ResolveOAuthLoginCredentialDispositionRefreshDisallowedValid {
                per_phase [Valid]
                on input ResolveOAuthLoginCredentialDisposition { credential_present, force_refresh, refresh_allowed }
                guard "needs_refresh_disallowed" {
                    !(self.credential_present && credential_present && force_refresh == false)
                    && refresh_allowed == false
                }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshDisallowed }
            }

            // Non-Valid phases never satisfy use-cached (the shell's
            // `lifecycle == Authorized` requires phase Valid), so the disposition
            // is purely refresh-or-error gated by `refresh_allowed`.
            transition ResolveOAuthLoginCredentialDispositionRefreshNonValid {
                per_phase [Expiring, Expired, Refreshing, ReauthRequired, Released]
                on input ResolveOAuthLoginCredentialDisposition { credential_present, force_refresh, refresh_allowed }
                guard "refresh_allowed" { refresh_allowed }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshRequired }
            }

            transition ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValid {
                per_phase [Expiring, Expired, Refreshing, ReauthRequired, Released]
                on input ResolveOAuthLoginCredentialDisposition { credential_present, force_refresh, refresh_allowed }
                guard "refresh_disallowed" { refresh_allowed == false }
                update {}
                to Valid
                emit CredentialUseAdmissionResolved { disposition: CredentialUseDisposition::RefreshDisallowed }
            }
        }
            }
    };
}

crate::auth_catalog_machine_dsl!("self", "catalog::dsl::auth_machine");
