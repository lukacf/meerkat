//! Phase 4b — OpenAI ChatGPT + Google Code Assist OAuth resolution
//! through the provider runtime.
//!
//! Covers the same choke-point as the Anthropic test: persisted tokens
//! → resolve returns an inline secret. Also verifies
//! the external_chatgpt_tokens path and the Google api_key_express path
//! (which routes through the simple-secret resolver).

#![cfg(all(not(target_arch = "wasm32"), feature = "oauth",))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration as ChronoDuration, Utc};

use meerkat_auth_core::auth_store::{
    EphemeralTokenStore, PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError,
    RefreshFn, TokenKey, TokenStore, TokenStoreError,
};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition, DslTransitionError,
    LeaseKey,
};
use meerkat_core::{
    AuthConstraints, AuthProfileConfig, BackendProfileConfig, BindingId, ConnectionRef,
    CredentialSourceSpec, ProviderBindingConfig, RealmConfigSection, RealmConnectionSet, RealmId,
};
use meerkat_llm_core::provider_runtime::{ProviderRuntimeRegistry, ResolverEnvironment};
use meerkat_openai::runtime::oauth as o_oauth;

struct FixedAuthLeaseHandle {
    snapshot: AuthLeaseSnapshot,
}

struct RecordingAuthLeaseHandle {
    snapshot: Mutex<AuthLeaseSnapshot>,
    acquired: Mutex<Vec<u64>>,
    completed_refreshes: Mutex<Vec<u64>>,
    refresh_failures: Mutex<Vec<bool>>,
    begin_refresh_failure_snapshot: Option<AuthLeaseSnapshot>,
    begin_refresh_pre_transition_snapshot: Mutex<Option<AuthLeaseSnapshot>>,
    complete_refresh_failure_snapshot: Mutex<Option<AuthLeaseSnapshot>>,
    acquire_if_snapshot_race: Mutex<Option<AuthLeaseSnapshot>>,
    mark_reauth_if_snapshot_race: Mutex<Option<AuthLeaseSnapshot>>,
}

struct StaticRefreshCoordinator {
    refreshed: PersistedTokens,
}

struct FailingRefreshCoordinator {
    error: RefreshError,
}

struct SequencedLoadTokenStore {
    first: PersistedTokens,
    later: PersistedTokens,
    loads: Mutex<usize>,
    on_later_load: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

struct MissingAfterFirstLoadTokenStore {
    first: PersistedTokens,
    loads: Mutex<usize>,
}

struct FailingSaveTokenStore {
    stored: Mutex<Option<PersistedTokens>>,
}

struct SaveRaceTokenStore {
    stored: Mutex<Option<PersistedTokens>>,
    on_save: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl FixedAuthLeaseHandle {
    fn reauth_required() -> Self {
        Self {
            snapshot: AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::ReauthRequired),
                expires_at: Some((Utc::now() + ChronoDuration::hours(1)).timestamp() as u64),
                generation: 7,
            },
        }
    }
}

impl RecordingAuthLeaseHandle {
    fn untracked() -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                generation: 0,
            }),
            acquired: Mutex::new(Vec::new()),
            completed_refreshes: Mutex::new(Vec::new()),
            refresh_failures: Mutex::new(Vec::new()),
            begin_refresh_failure_snapshot: None,
            begin_refresh_pre_transition_snapshot: Mutex::new(None),
            complete_refresh_failure_snapshot: Mutex::new(None),
            acquire_if_snapshot_race: Mutex::new(None),
            mark_reauth_if_snapshot_race: Mutex::new(None),
        }
    }

    fn valid(expires_at: chrono::DateTime<Utc>) -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(expires_at.timestamp() as u64),
                generation: 3,
            }),
            acquired: Mutex::new(Vec::new()),
            completed_refreshes: Mutex::new(Vec::new()),
            refresh_failures: Mutex::new(Vec::new()),
            begin_refresh_failure_snapshot: None,
            begin_refresh_pre_transition_snapshot: Mutex::new(None),
            complete_refresh_failure_snapshot: Mutex::new(None),
            acquire_if_snapshot_race: Mutex::new(None),
            mark_reauth_if_snapshot_race: Mutex::new(None),
        }
    }

    fn begin_refresh_race(expires_at: chrono::DateTime<Utc>) -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(expires_at.timestamp() as u64),
                generation: 3,
            }),
            acquired: Mutex::new(Vec::new()),
            completed_refreshes: Mutex::new(Vec::new()),
            refresh_failures: Mutex::new(Vec::new()),
            begin_refresh_failure_snapshot: Some(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Refreshing),
                expires_at: Some(expires_at.timestamp() as u64),
                generation: 4,
            }),
            begin_refresh_pre_transition_snapshot: Mutex::new(None),
            complete_refresh_failure_snapshot: Mutex::new(None),
            acquire_if_snapshot_race: Mutex::new(None),
            mark_reauth_if_snapshot_race: Mutex::new(None),
        }
    }

    fn begin_refresh_race_completed(
        expires_at: chrono::DateTime<Utc>,
        completed_expires_at: chrono::DateTime<Utc>,
    ) -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(expires_at.timestamp() as u64),
                generation: 3,
            }),
            acquired: Mutex::new(Vec::new()),
            completed_refreshes: Mutex::new(Vec::new()),
            refresh_failures: Mutex::new(Vec::new()),
            begin_refresh_failure_snapshot: Some(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(completed_expires_at.timestamp() as u64),
                generation: 4,
            }),
            begin_refresh_pre_transition_snapshot: Mutex::new(None),
            complete_refresh_failure_snapshot: Mutex::new(None),
            acquire_if_snapshot_race: Mutex::new(None),
            mark_reauth_if_snapshot_race: Mutex::new(None),
        }
    }

    fn begin_refresh_stale_snapshot_race_to_valid(
        stale_expires_at: chrono::DateTime<Utc>,
        completed_expires_at: chrono::DateTime<Utc>,
    ) -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(stale_expires_at.timestamp() as u64),
                generation: 3,
            }),
            acquired: Mutex::new(Vec::new()),
            completed_refreshes: Mutex::new(Vec::new()),
            refresh_failures: Mutex::new(Vec::new()),
            begin_refresh_failure_snapshot: None,
            begin_refresh_pre_transition_snapshot: Mutex::new(Some(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(completed_expires_at.timestamp() as u64),
                generation: 4,
            })),
            complete_refresh_failure_snapshot: Mutex::new(None),
            acquire_if_snapshot_race: Mutex::new(None),
            mark_reauth_if_snapshot_race: Mutex::new(None),
        }
    }

    fn refreshing(expires_at: chrono::DateTime<Utc>) -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Refreshing),
                expires_at: Some(expires_at.timestamp() as u64),
                generation: 5,
            }),
            acquired: Mutex::new(Vec::new()),
            completed_refreshes: Mutex::new(Vec::new()),
            refresh_failures: Mutex::new(Vec::new()),
            begin_refresh_failure_snapshot: None,
            begin_refresh_pre_transition_snapshot: Mutex::new(None),
            complete_refresh_failure_snapshot: Mutex::new(None),
            acquire_if_snapshot_race: Mutex::new(None),
            mark_reauth_if_snapshot_race: Mutex::new(None),
        }
    }

    fn acquired(&self) -> Vec<u64> {
        self.acquired.lock().unwrap().clone()
    }

    fn completed_refreshes(&self) -> Vec<u64> {
        self.completed_refreshes.lock().unwrap().clone()
    }

    fn refresh_failures(&self) -> Vec<bool> {
        self.refresh_failures.lock().unwrap().clone()
    }

    fn phase(&self) -> Option<AuthLeasePhase> {
        self.snapshot.lock().unwrap().phase
    }

    fn replace_snapshot(&self, phase: AuthLeasePhase, expires_at: Option<u64>) {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = Some(phase);
        snapshot.expires_at = expires_at;
        snapshot.generation += 1;
    }

    fn race_next_conditional_acquire(&self, snapshot: AuthLeaseSnapshot) {
        *self.acquire_if_snapshot_race.lock().unwrap() = Some(snapshot);
    }

    fn race_next_conditional_mark_reauth(&self, snapshot: AuthLeaseSnapshot) {
        *self.mark_reauth_if_snapshot_race.lock().unwrap() = Some(snapshot);
    }

    fn fail_next_complete_refresh_with(&self, snapshot: AuthLeaseSnapshot) {
        *self.complete_refresh_failure_snapshot.lock().unwrap() = Some(snapshot);
    }
}

impl AuthLeaseHandle for FixedAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        _expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        panic!("managed OAuth must not reacquire over reauth-required lease truth")
    }

    fn acquire_lease_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        _expected: &AuthLeaseSnapshot,
        _expires_at: u64,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        panic!("managed OAuth must not conditionally reacquire over reauth-required lease truth")
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        panic!("managed OAuth must not mark expiring over reauth-required lease truth")
    }

    fn begin_refresh(
        &self,
        _lease_key: &LeaseKey,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        panic!("managed OAuth must not refresh over reauth-required lease truth")
    }

    fn begin_refresh_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        _expected: &AuthLeaseSnapshot,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        panic!("managed OAuth must not conditionally refresh over reauth-required lease truth")
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        _new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        panic!("managed OAuth must not complete refresh over reauth-required lease truth")
    }

    fn complete_refresh_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        _expected: &AuthLeaseSnapshot,
        _new_expires_at: u64,
        _now: u64,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        panic!(
            "managed OAuth must not conditionally complete refresh over reauth-required lease truth"
        )
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        _permanent: bool,
    ) -> Result<(), DslTransitionError> {
        panic!("managed OAuth must not report refresh failure over reauth-required lease truth")
    }

    fn refresh_failed_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        _expected: &AuthLeaseSnapshot,
        _permanent: bool,
    ) -> Result<bool, DslTransitionError> {
        panic!("managed OAuth must not report refresh failure over reauth-required lease truth")
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn mark_reauth_required_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        expected: &AuthLeaseSnapshot,
    ) -> Result<bool, DslTransitionError> {
        Ok(matches!(
            expected.phase,
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
        ))
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.snapshot.clone()
    }
}

impl AuthLeaseHandle for RecordingAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        self.acquired.lock().unwrap().push(expires_at);
        self.replace_snapshot(AuthLeasePhase::Valid, lease_expires_at_arg(expires_at));
        Ok(AuthLeaseTransition {
            generation: self.snapshot.lock().unwrap().generation,
        })
    }

    fn acquire_lease_if_snapshot(
        &self,
        lease_key: &LeaseKey,
        expected: &AuthLeaseSnapshot,
        expires_at: u64,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        if let Some(snapshot) = self.acquire_if_snapshot_race.lock().unwrap().take() {
            *self.snapshot.lock().unwrap() = snapshot;
        }
        if self.snapshot(lease_key) != *expected {
            return Ok(None);
        }
        self.acquire_lease(lease_key, expires_at).map(Some)
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let expires_at = self.snapshot.lock().unwrap().expires_at;
        self.replace_snapshot(AuthLeasePhase::Expiring, expires_at);
        Ok(())
    }

    fn begin_refresh(
        &self,
        _lease_key: &LeaseKey,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        if let Some(snapshot) = self
            .begin_refresh_pre_transition_snapshot
            .lock()
            .unwrap()
            .take()
        {
            *self.snapshot.lock().unwrap() = snapshot;
        }
        if let Some(snapshot) = &self.begin_refresh_failure_snapshot {
            *self.snapshot.lock().unwrap() = snapshot.clone();
            return Err(DslTransitionError::new(
                "begin_refresh",
                "another actor moved the lease into refreshing",
            ));
        }
        let expires_at = self.snapshot.lock().unwrap().expires_at;
        if !matches!(
            self.snapshot.lock().unwrap().phase,
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
        ) {
            return Err(DslTransitionError::new(
                "begin_refresh",
                "lease is not valid or expiring",
            ));
        }
        self.replace_snapshot(AuthLeasePhase::Refreshing, expires_at);
        Ok(AuthLeaseTransition {
            generation: self.snapshot.lock().unwrap().generation,
        })
    }

    fn begin_refresh_if_snapshot(
        &self,
        lease_key: &LeaseKey,
        expected: &AuthLeaseSnapshot,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        if let Some(snapshot) = self
            .begin_refresh_pre_transition_snapshot
            .lock()
            .unwrap()
            .take()
        {
            *self.snapshot.lock().unwrap() = snapshot;
        }
        if let Some(snapshot) = &self.begin_refresh_failure_snapshot {
            *self.snapshot.lock().unwrap() = snapshot.clone();
            return Ok(None);
        }
        if self.snapshot(lease_key) != *expected
            || !matches!(
                expected.phase,
                Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
            )
        {
            return Ok(None);
        }
        self.begin_refresh(lease_key).map(Some)
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        if let Some(snapshot) = self
            .complete_refresh_failure_snapshot
            .lock()
            .unwrap()
            .take()
        {
            *self.snapshot.lock().unwrap() = snapshot;
            return Err(DslTransitionError::new(
                "complete_refresh",
                "another actor moved the lease before completion",
            ));
        }
        self.completed_refreshes
            .lock()
            .unwrap()
            .push(new_expires_at);
        self.replace_snapshot(AuthLeasePhase::Valid, lease_expires_at_arg(new_expires_at));
        Ok(AuthLeaseTransition {
            generation: self.snapshot.lock().unwrap().generation,
        })
    }

    fn complete_refresh_if_snapshot(
        &self,
        lease_key: &LeaseKey,
        expected: &AuthLeaseSnapshot,
        new_expires_at: u64,
        now: u64,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        if self.snapshot(lease_key) != *expected
            || expected.phase != Some(AuthLeasePhase::Refreshing)
        {
            return Ok(None);
        }
        self.complete_refresh(lease_key, new_expires_at, now)
            .map(Some)
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        permanent: bool,
    ) -> Result<(), DslTransitionError> {
        self.refresh_failures.lock().unwrap().push(permanent);
        let phase = if permanent {
            AuthLeasePhase::ReauthRequired
        } else {
            AuthLeasePhase::Expiring
        };
        let expires_at = self.snapshot.lock().unwrap().expires_at;
        self.replace_snapshot(phase, expires_at);
        Ok(())
    }

    fn refresh_failed_if_snapshot(
        &self,
        lease_key: &LeaseKey,
        expected: &AuthLeaseSnapshot,
        permanent: bool,
    ) -> Result<bool, DslTransitionError> {
        if self.snapshot(lease_key) != *expected
            || expected.phase != Some(AuthLeasePhase::Refreshing)
        {
            return Ok(false);
        }
        self.refresh_failed(lease_key, permanent)?;
        Ok(true)
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let expires_at = self.snapshot.lock().unwrap().expires_at;
        self.replace_snapshot(AuthLeasePhase::ReauthRequired, expires_at);
        Ok(())
    }

    fn mark_reauth_required_if_snapshot(
        &self,
        lease_key: &LeaseKey,
        expected: &AuthLeaseSnapshot,
    ) -> Result<bool, DslTransitionError> {
        if let Some(snapshot) = self.mark_reauth_if_snapshot_race.lock().unwrap().take() {
            *self.snapshot.lock().unwrap() = snapshot;
        }
        if self.snapshot(lease_key) != *expected
            || !matches!(
                expected.phase,
                Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
            )
        {
            return Ok(false);
        }
        self.mark_reauth_required(lease_key)?;
        Ok(true)
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = None;
        snapshot.expires_at = None;
        snapshot.generation += 1;
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.snapshot.lock().unwrap().clone()
    }
}

impl SequencedLoadTokenStore {
    fn new(first: PersistedTokens, later: PersistedTokens) -> Self {
        Self {
            first,
            later,
            loads: Mutex::new(0),
            on_later_load: Mutex::new(None),
        }
    }

    fn new_with_later_load_callback(
        first: PersistedTokens,
        later: PersistedTokens,
        on_later_load: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            first,
            later,
            loads: Mutex::new(0),
            on_later_load: Mutex::new(Some(on_later_load)),
        }
    }

    fn loads(&self) -> usize {
        *self.loads.lock().unwrap()
    }
}

impl MissingAfterFirstLoadTokenStore {
    fn new(first: PersistedTokens) -> Self {
        Self {
            first,
            loads: Mutex::new(0),
        }
    }

    fn loads(&self) -> usize {
        *self.loads.lock().unwrap()
    }
}

impl FailingSaveTokenStore {
    fn new(stored: PersistedTokens) -> Self {
        Self {
            stored: Mutex::new(Some(stored)),
        }
    }
}

impl SaveRaceTokenStore {
    fn new(stored: PersistedTokens, on_save: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            stored: Mutex::new(Some(stored)),
            on_save: Mutex::new(Some(on_save)),
        }
    }
}

#[async_trait::async_trait]
impl TokenStore for SequencedLoadTokenStore {
    async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let mut loads = self.loads.lock().unwrap();
        *loads += 1;
        if *loads == 1 {
            Ok(Some(self.first.clone()))
        } else {
            drop(loads);
            if let Some(on_later_load) = self.on_later_load.lock().unwrap().take() {
                on_later_load();
            }
            Ok(Some(self.later.clone()))
        }
    }

    async fn save(
        &self,
        _key: &TokenKey,
        _tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        Ok(())
    }

    async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
        Ok(())
    }

    async fn clear_if_current(
        &self,
        _key: &TokenKey,
        _expected: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        Ok(false)
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        Ok(Vec::new())
    }

    fn backend_name(&self) -> &'static str {
        "sequenced-load"
    }
}

#[async_trait::async_trait]
impl TokenStore for MissingAfterFirstLoadTokenStore {
    async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let mut loads = self.loads.lock().unwrap();
        *loads += 1;
        if *loads == 1 {
            Ok(Some(self.first.clone()))
        } else {
            Ok(None)
        }
    }

    async fn save(
        &self,
        _key: &TokenKey,
        _tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        Ok(())
    }

    async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
        Ok(())
    }

    async fn clear_if_current(
        &self,
        _key: &TokenKey,
        _expected: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        Ok(false)
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        Ok(Vec::new())
    }

    fn backend_name(&self) -> &'static str {
        "missing-after-first-load"
    }
}

#[async_trait::async_trait]
impl TokenStore for FailingSaveTokenStore {
    async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        Ok(self.stored.lock().unwrap().clone())
    }

    async fn save(
        &self,
        _key: &TokenKey,
        _tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Unavailable(
            "programmed save failure".into(),
        ))
    }

    async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
        *self.stored.lock().unwrap() = None;
        Ok(())
    }

    async fn clear_if_current(
        &self,
        _key: &TokenKey,
        expected: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let mut stored = self.stored.lock().unwrap();
        if stored.as_ref() != Some(expected) {
            return Ok(false);
        }
        *stored = None;
        Ok(true)
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        Ok(Vec::new())
    }

    fn backend_name(&self) -> &'static str {
        "failing-save"
    }
}

#[async_trait::async_trait]
impl TokenStore for SaveRaceTokenStore {
    async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        Ok(self.stored.lock().unwrap().clone())
    }

    async fn save(&self, _key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        *self.stored.lock().unwrap() = Some(tokens.clone());
        if let Some(on_save) = self.on_save.lock().unwrap().take() {
            on_save();
        }
        Ok(())
    }

    async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
        *self.stored.lock().unwrap() = None;
        Ok(())
    }

    async fn clear_if_current(
        &self,
        _key: &TokenKey,
        expected: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let mut stored = self.stored.lock().unwrap();
        if stored.as_ref() != Some(expected) {
            return Ok(false);
        }
        *stored = None;
        Ok(true)
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        Ok(Vec::new())
    }

    fn backend_name(&self) -> &'static str {
        "save-race"
    }
}

#[async_trait::async_trait]
impl RefreshCoordinator for StaticRefreshCoordinator {
    async fn with_refresh(
        &self,
        _key: TokenKey,
        _refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        Ok(self.refreshed.clone())
    }
}

#[async_trait::async_trait]
impl RefreshCoordinator for FailingRefreshCoordinator {
    async fn with_refresh(
        &self,
        _key: TokenKey,
        _refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        Err(self.error.clone())
    }
}

fn lease_expires_at_arg(expires_at: u64) -> Option<u64> {
    (expires_at != u64::MAX).then_some(expires_at)
}

fn openai_realm(backend_kind: &str, auth_method: &str) -> RealmConnectionSet {
    openai_realm_with_constraints(
        backend_kind,
        auth_method,
        AuthConstraints {
            allow_interactive_login: true,
            ..Default::default()
        },
    )
}

fn openai_realm_with_constraints(
    backend_kind: &str,
    auth_method: &str,
    constraints: AuthConstraints,
) -> RealmConnectionSet {
    let mut backend = BTreeMap::new();
    backend.insert(
        backend_kind.into(),
        BackendProfileConfig {
            provider: "openai".into(),
            backend_kind: backend_kind.into(),
            base_url: None,
            options: serde_json::json!({"realm_id": "dev"}),
        },
    );
    let mut auth = BTreeMap::new();
    auth.insert(
        "chatgpt_auth".into(),
        AuthProfileConfig {
            provider: "openai".into(),
            auth_method: auth_method.into(),
            source: CredentialSourceSpec::PlatformDefault,
            constraints,
            metadata_defaults: Default::default(),
        },
    );
    let mut binding = BTreeMap::new();
    binding.insert(
        "default_chatgpt".into(),
        ProviderBindingConfig {
            backend_profile: backend_kind.into(),
            auth_profile: "chatgpt_auth".into(),
            default_model: None,
            policy: Default::default(),
        },
    );
    RealmConnectionSet::from_config(
        "dev",
        &RealmConfigSection {
            backend,
            auth,
            binding,
            default_binding: Some("default_chatgpt".into()),
        },
    )
    .unwrap()
}

fn default_connection_ref() -> ConnectionRef {
    ConnectionRef {
        realm: RealmId::parse("dev").expect("valid realm"),
        binding: BindingId::parse("default_chatgpt").expect("valid binding"),
        profile: None,
    }
}

// --- OpenAI managed_chatgpt_oauth ------------------------------------

#[tokio::test]
async fn openai_managed_chatgpt_oauth_fresh_token_resolves() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("fresh ChatGPT tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-chatgpt-access".to_string()),
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_reauth_required_does_not_return_cached_token() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(Arc::new(FixedAuthLeaseHandle::reauth_required()));
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("AuthMachine reauth-required truth must block cached TokenStore access");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_stale_lease_forces_refresh_and_publishes_new_expiry() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("cached-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let refresh_coord = Arc::new(StaticRefreshCoordinator {
        refreshed: refreshed.clone(),
    });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("stale AuthMachine lease should refresh managed OAuth token");

    assert_eq!(
        connection.resolved_secret(),
        Some("refreshed-chatgpt-access".to_string()),
    );
    assert_eq!(
        connection.auth_lease.expires_at(),
        Some(refreshed_expires_at),
        "resolved lease must carry the refreshed token expiry"
    );
    assert_eq!(
        auth_lease.completed_refreshes(),
        vec![refreshed_expires_at.timestamp() as u64],
        "AuthMachine complete_refresh must receive the refreshed expiry, not the stale persisted expiry"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_saves_refreshed_tokens_before_publishing_valid_lease() {
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("cached-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let observed_lease = Arc::clone(&auth_lease);
    let store = Arc::new(SaveRaceTokenStore::new(
        persisted.clone(),
        Box::new(move || {
            assert_eq!(
                observed_lease.phase(),
                Some(AuthLeasePhase::Refreshing),
                "TokenStore save must happen before AuthMachine publishes the refreshed lease as Valid"
            );
        }),
    ));

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let refresh_coord = Arc::new(StaticRefreshCoordinator { refreshed });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("managed OAuth refresh should save before publishing Valid lease");

    assert_eq!(
        connection.resolved_secret(),
        Some("refreshed-chatgpt-access".to_string())
    );
    assert_eq!(auth_lease.phase(), Some(AuthLeasePhase::Valid));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_cleans_saved_refresh_when_authmachine_completion_loses_race()
{
    let store = Arc::new(EphemeralTokenStore::new());
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("cached-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store.save(&key, &persisted).await.unwrap();

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    auth_lease.fail_next_complete_refresh_with(AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::Released),
        expires_at: None,
        generation: 99,
    });
    let refresh_coord = Arc::new(StaticRefreshCoordinator { refreshed });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("AuthMachine completion race must fail provider resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::SourceResolutionFailed(_)
        ),
        "got {err:?}"
    );
    assert_eq!(
        store.load(&key).await.unwrap(),
        None,
        "refreshed token material must be cleaned up when AuthMachine rejects completion"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::Released),
        "newer AuthMachine truth must remain authoritative"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_save_failure_fails_owner_refresh_before_valid_publication() {
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("cached-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let store = Arc::new(FailingSaveTokenStore::new(persisted.clone()));

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let refresh_coord = Arc::new(StaticRefreshCoordinator { refreshed });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("TokenStore save failure must fail provider resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::SourceResolutionFailed(_)
        ),
        "got {err:?}"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::Expiring),
        "AuthMachine must not publish a valid refreshed lease when refreshed tokens cannot be saved"
    );
    assert_eq!(
        auth_lease.refresh_failures(),
        vec![false],
        "save failure should fail only the owner refresh, as a retryable failure"
    );
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("cached-chatgpt-access")
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_post_save_lease_race_clears_stale_refresh() {
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("cached-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let lease_key = LeaseKey::from_connection_ref(&default_connection_ref());
    let race_handle = Arc::clone(&auth_lease);
    let store = Arc::new(SaveRaceTokenStore::new(
        persisted.clone(),
        Box::new(move || {
            race_handle.release_lease(&lease_key).unwrap();
        }),
    ));

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let refresh_coord = Arc::new(StaticRefreshCoordinator { refreshed });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("post-save AuthMachine race must fail provider resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::SourceResolutionFailed(_)
        ),
        "got {err:?}"
    );
    assert_eq!(
        store.load(&key).await.unwrap(),
        None,
        "stale refreshed token material must be cleaned up when AuthMachine truth changes during save"
    );
    assert_eq!(
        auth_lease.phase(),
        None,
        "newer AuthMachine release must remain authoritative"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_missing_expiry_remains_authmachine_non_expiring() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("non-expiring-chatgpt-access".into()),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::untracked());
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("missing expiry should remain non-expiring AuthMachine truth");

    assert_eq!(
        connection.resolved_secret(),
        Some("non-expiring-chatgpt-access".to_string()),
    );
    assert_eq!(connection.auth_lease.expires_at(), None);
    let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&default_connection_ref()));
    assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
    assert_eq!(snapshot.expires_at, None);
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_missing_cached_access_refreshes_when_lease_is_fresh() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: None,
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() + ChronoDuration::hours(1),
    ));
    let refresh_coord = Arc::new(StaticRefreshCoordinator { refreshed });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("missing cached access should refresh even when lease expiry is fresh");

    assert_eq!(
        connection.resolved_secret(),
        Some("refreshed-chatgpt-access".to_string()),
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_fresh_authmachine_reloads_store_before_cached_return() {
    let stale = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-stale".into()),
        metadata: serde_json::Value::Null,
    };
    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("owner-refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        account_id: Some("acct-refreshed".into()),
        ..stale.clone()
    };
    let store = Arc::new(SequencedLoadTokenStore::new(stale, refreshed));
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(refreshed_expires_at));

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_auth_lease_handle(auth_lease);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("fresh AuthMachine truth should resolve from current TokenStore material");

    assert_eq!(
        connection.resolved_secret(),
        Some("owner-refreshed-chatgpt-access".to_string()),
        "cached return must not use TokenStore material loaded before the owner refresh completed"
    );
    assert_eq!(
        connection.auth_lease.expires_at(),
        Some(refreshed_expires_at)
    );
    assert_eq!(
        connection.auth_lease.metadata().account_id.as_deref(),
        Some("acct-refreshed"),
        "metadata must come from the same reloaded TokenStore material as the access token"
    );
    match &connection.auth_lease.metadata().provider_metadata {
        Some(meerkat_core::ProviderAuthMetadata::OpenAi(metadata)) => {
            assert_eq!(metadata.account_id.as_deref(), Some("acct-refreshed"));
        }
        other => panic!("expected OpenAI metadata from reloaded tokens, got {other:?}"),
    }
    assert_eq!(
        store.loads(),
        2,
        "resolver should reload TokenStore under fresh AuthMachine truth"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_reauth_race_after_reload_does_not_reacquire_from_store() {
    let lease_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(1)).timestamp(),
        0,
    )
    .unwrap();
    let token_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let stale = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("lease-current-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(lease_expires_at),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let newer = PersistedTokens {
        primary_secret: Some("newer-store-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(token_expires_at),
        last_refresh: Some(Utc::now()),
        ..stale.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(lease_expires_at));
    let lease_for_store = auth_lease.clone();
    let store = Arc::new(SequencedLoadTokenStore::new_with_later_load_callback(
        stale,
        newer,
        Box::new(move || {
            lease_for_store.replace_snapshot(
                AuthLeasePhase::ReauthRequired,
                Some(lease_expires_at.timestamp() as u64),
            );
        }),
    ));

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("stale TokenStore freshness must not reacquire over reauth-required truth");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::ReauthRequired),
        "AuthMachine reauth-required truth must survive a stale resolver snapshot"
    );
    assert_eq!(
        auth_lease.acquired(),
        Vec::<u64>::new(),
        "resolver must not let a newer TokenStore expiry promote AuthMachine truth"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_none_snapshot_race_does_not_acquire_stale_loaded_token() {
    let stale_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(1)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let stale = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(stale_expires_at),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let refreshed = PersistedTokens {
        primary_secret: Some("owner-refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..stale.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::untracked());
    let lease_for_store = auth_lease.clone();
    let store = Arc::new(SequencedLoadTokenStore::new_with_later_load_callback(
        stale,
        refreshed,
        Box::new(move || {
            lease_for_store.replace_snapshot(
                AuthLeasePhase::Valid,
                Some(refreshed_expires_at.timestamp() as u64),
            );
        }),
    ));

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("resolver should observe the owner lease before acquiring stale material");

    assert_eq!(
        connection.resolved_secret(),
        Some("owner-refreshed-chatgpt-access".to_string()),
    );
    assert_eq!(
        auth_lease.acquired(),
        Vec::<u64>::new(),
        "resolver must not acquire a stale lease after another owner publishes truth"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_none_snapshot_conditional_acquire_race_preserves_reauth() {
    let store = Arc::new(EphemeralTokenStore::new());
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(1)).timestamp(),
        0,
    )
    .unwrap();
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(expires_at),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store.save(&key, &persisted).await.unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::untracked());
    auth_lease.race_next_conditional_acquire(AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::ReauthRequired),
        expires_at: Some(expires_at.timestamp() as u64),
        generation: 1,
    });
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("conditional acquire must not publish Valid over concurrent reauth truth");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    assert_eq!(auth_lease.phase(), Some(AuthLeasePhase::ReauthRequired));
    assert_eq!(
        auth_lease.acquired(),
        Vec::<u64>::new(),
        "conditional acquire must leave the concurrent AuthMachine phase intact"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_joining_refresh_waits_for_owner_and_reloads_store() {
    let store = Arc::new(EphemeralTokenStore::new());
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store.save(&key, &persisted).await.unwrap();

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("owner-refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::refreshing(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let owner_lease = auth_lease.clone();
    let owner_store = store.clone();
    let owner_key = key.clone();
    let owner_lease_key = LeaseKey::from_connection_ref(&default_connection_ref());
    let owner_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        owner_store.save(&owner_key, &refreshed).await.unwrap();
        owner_lease
            .complete_refresh(
                &owner_lease_key,
                refreshed_expires_at.timestamp() as u64,
                Utc::now().timestamp() as u64,
            )
            .unwrap();
    });
    let refresh_coord = Arc::new(FailingRefreshCoordinator {
        error: RefreshError::Refresh(
            "token endpoint error: status=400 body={\"error\":\"invalid_grant\"}".into(),
        ),
    });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("joining resolver must wait for the owner refresh and reload TokenStore");
    owner_task.await.unwrap();

    assert_eq!(
        connection.resolved_secret(),
        Some("owner-refreshed-chatgpt-access".to_string()),
    );
    assert_eq!(
        auth_lease.refresh_failures(),
        Vec::<bool>::new(),
        "joining resolver must not report refresh_failed for an owner refresh it did not start"
    );
    assert_eq!(auth_lease.phase(), Some(AuthLeasePhase::Valid));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_no_refresh_does_not_join_owner_refresh() {
    let store = Arc::new(EphemeralTokenStore::new());
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store.save(&key, &persisted).await.unwrap();

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("owner-refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::refreshing(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let owner_lease = auth_lease.clone();
    let owner_store = store.clone();
    let owner_key = key.clone();
    let owner_lease_key = LeaseKey::from_connection_ref(&default_connection_ref());
    let owner_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        owner_store.save(&owner_key, &refreshed).await.unwrap();
        owner_lease
            .complete_refresh(
                &owner_lease_key,
                refreshed_expires_at.timestamp() as u64,
                Utc::now().timestamp() as u64,
            )
            .unwrap();
    });

    let realm = openai_realm_with_constraints(
        "chatgpt_backend",
        "managed_chatgpt_oauth",
        AuthConstraints {
            allow_interactive_login: true,
            allow_refresh: false,
            ..Default::default()
        },
    );
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("no-refresh bindings must not join an owner refresh");
    owner_task.await.unwrap();

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    assert_eq!(
        auth_lease.refresh_failures(),
        Vec::<bool>::new(),
        "no-refresh resolver must not fail an owner refresh it did not start"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::Valid),
        "owner refresh should remain free to complete independently"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_begin_refresh_race_waits_for_owner_and_reloads_store() {
    let store = Arc::new(EphemeralTokenStore::new());
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store.save(&key, &persisted).await.unwrap();

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("owner-race-refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::begin_refresh_race(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let owner_lease = auth_lease.clone();
    let owner_store = store.clone();
    let owner_key = key.clone();
    let owner_lease_key = LeaseKey::from_connection_ref(&default_connection_ref());
    let owner_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        owner_store.save(&owner_key, &refreshed).await.unwrap();
        owner_lease
            .complete_refresh(
                &owner_lease_key,
                refreshed_expires_at.timestamp() as u64,
                Utc::now().timestamp() as u64,
            )
            .unwrap();
    });
    let refresh_coord = Arc::new(FailingRefreshCoordinator {
        error: RefreshError::Refresh(
            "token endpoint error: status=400 body={\"error\":\"invalid_grant\"}".into(),
        ),
    });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("begin-refresh race must wait for the owner refresh and reload TokenStore");
    owner_task.await.unwrap();

    assert_eq!(
        connection.resolved_secret(),
        Some("owner-race-refreshed-chatgpt-access".to_string()),
    );
    assert_eq!(
        auth_lease.refresh_failures(),
        Vec::<bool>::new(),
        "begin-refresh loser must not report refresh_failed for the owner refresh"
    );
    assert_eq!(auth_lease.phase(), Some(AuthLeasePhase::Valid));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_begin_refresh_race_completed_before_followup_snapshot_reloads_store()
 {
    let stale_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() - ChronoDuration::minutes(5)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let stale = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(stale_expires_at),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let refreshed = PersistedTokens {
        primary_secret: Some("owner-race-refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..stale.clone()
    };
    let store = Arc::new(SequencedLoadTokenStore::new(stale, refreshed));
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::begin_refresh_race_completed(
        stale_expires_at,
        refreshed_expires_at,
    ));
    let refresh_coord = Arc::new(FailingRefreshCoordinator {
        error: RefreshError::Refresh(
            "token endpoint error: status=400 body={\"error\":\"invalid_grant\"}".into(),
        ),
    });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("begin-refresh loser should reload if owner completed before follow-up snapshot");

    assert_eq!(
        connection.resolved_secret(),
        Some("owner-race-refreshed-chatgpt-access".to_string()),
    );
    assert_eq!(
        auth_lease.refresh_failures(),
        Vec::<bool>::new(),
        "begin-refresh loser must not fail the owner-completed refresh"
    );
    assert_eq!(auth_lease.phase(), Some(AuthLeasePhase::Valid));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_begin_refresh_stale_snapshot_rechecks_before_owner_claim() {
    let stale_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() - ChronoDuration::minutes(5)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let stale = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(stale_expires_at),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let refreshed = PersistedTokens {
        primary_secret: Some("owner-won-before-begin-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..stale.clone()
    };
    let store = Arc::new(SequencedLoadTokenStore::new(stale, refreshed));
    let auth_lease = Arc::new(
        RecordingAuthLeaseHandle::begin_refresh_stale_snapshot_race_to_valid(
            stale_expires_at,
            refreshed_expires_at,
        ),
    );
    let refresh_coord = Arc::new(FailingRefreshCoordinator {
        error: RefreshError::Refresh(
            "token endpoint error: provider refresh should not run".into(),
        ),
    });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("stale begin-refresh snapshot must recheck owner lease truth and reload");

    assert_eq!(
        connection.resolved_secret(),
        Some("owner-won-before-begin-chatgpt-access".to_string()),
    );
    assert_eq!(
        auth_lease.refresh_failures(),
        Vec::<bool>::new(),
        "stale begin-refresh loser must not report refresh_failed"
    );
    assert_eq!(auth_lease.phase(), Some(AuthLeasePhase::Valid));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_no_refresh_missing_reloaded_store_marks_reauth_required() {
    let expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(1)).timestamp(),
        0,
    )
    .unwrap();
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(expires_at),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };

    let store = Arc::new(MissingAfterFirstLoadTokenStore::new(persisted));
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(expires_at));
    let realm = openai_realm_with_constraints(
        "chatgpt_backend",
        "managed_chatgpt_oauth",
        AuthConstraints {
            allow_interactive_login: true,
            allow_refresh: false,
            ..Default::default()
        },
    );
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("missing reloaded TokenStore material without refresh must fail resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::ReauthRequired),
        "fresh AuthMachine truth must not survive after TokenStore material disappears"
    );
    assert_eq!(
        store.loads(),
        2,
        "resolver should reload TokenStore under fresh AuthMachine truth"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_no_refresh_missing_access_marks_reauth_required() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: None,
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::untracked());
    let realm = openai_realm_with_constraints(
        "chatgpt_backend",
        "managed_chatgpt_oauth",
        AuthConstraints {
            allow_interactive_login: true,
            allow_refresh: false,
            ..Default::default()
        },
    );
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("missing cached access without refresh must fail resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::ReauthRequired),
        "AuthMachine must not remain valid after resolution proves no credential material is available"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_no_refresh_mark_race_does_not_poison_owner_refresh() {
    let store = Arc::new(EphemeralTokenStore::new());
    let expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(1)).timestamp(),
        0,
    )
    .unwrap();
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: None,
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(expires_at),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(expires_at));
    auth_lease.race_next_conditional_mark_reauth(AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::Refreshing),
        expires_at: Some(expires_at.timestamp() as u64),
        generation: 4,
    });
    let realm = openai_realm_with_constraints(
        "chatgpt_backend",
        "managed_chatgpt_oauth",
        AuthConstraints {
            allow_interactive_login: true,
            allow_refresh: false,
            ..Default::default()
        },
    );
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("no-refresh conditional reauth must not poison a concurrent owner refresh");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::Refreshing),
        "no-refresh resolver must not mark a concurrent owner refresh reauth-required"
    );
    assert_eq!(
        auth_lease.refresh_failures(),
        Vec::<bool>::new(),
        "no-refresh resolver must not fail an owner refresh it did not start"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_no_refresh_mismatched_store_marks_reauth_required() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(1)).timestamp(),
        0,
    )
    .unwrap();
    let lease_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(persisted_expires_at),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(lease_expires_at));
    let realm = openai_realm_with_constraints(
        "chatgpt_backend",
        "managed_chatgpt_oauth",
        AuthConstraints {
            allow_interactive_login: true,
            allow_refresh: false,
            ..Default::default()
        },
    );
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("mismatched TokenStore material without refresh must fail resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::ReauthRequired),
        "AuthMachine must not remain valid after no-refresh material disagrees with lease truth"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_no_refresh_stale_access_marks_reauth_required() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::untracked());
    let realm = openai_realm_with_constraints(
        "chatgpt_backend",
        "managed_chatgpt_oauth",
        AuthConstraints {
            allow_interactive_login: true,
            allow_refresh: false,
            ..Default::default()
        },
    );
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("stale cached access without refresh must fail resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    assert_eq!(
        auth_lease.phase(),
        Some(AuthLeasePhase::ReauthRequired),
        "AuthMachine must not remain valid after a stale no-refresh resolution"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_invalid_grant_marks_reauth_required() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("cached-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let refresh_coord = Arc::new(FailingRefreshCoordinator {
        error: RefreshError::Refresh(
            "token endpoint error: status=400 body={\"error\":\"invalid_grant\"}".into(),
        ),
    });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("invalid_grant should fail resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::SourceResolutionFailed(_)
        ),
        "got {err:?}"
    );
    assert_eq!(auth_lease.phase(), Some(AuthLeasePhase::ReauthRequired));
}

#[tokio::test]
async fn openai_external_chatgpt_tokens_returns_persisted_access() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ExternalTokens,
        primary_secret: Some("externally-managed-access".into()),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(Utc::now()),
        scopes: vec![],
        account_id: Some("acct-ext".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "external_chatgpt_tokens");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("external tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("externally-managed-access".to_string()),
    );
}

#[tokio::test]
async fn openai_chatgpt_oauth_missing_tokens_surfaces_interactive_login_required() {
    let store = Arc::new(EphemeralTokenStore::new());
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::InteractiveLoginRequired
            )
        ),
        "got {err:?}"
    );
}
