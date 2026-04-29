//! Auth lifecycle publication helpers.
//!
//! TokenStore owns credential material. AuthMachine owns lifecycle state.
//! Surfaces that write or clear credentials use these helpers so the public
//! status path observes the machine-owned lease instead of deriving phase from
//! persisted token bytes.

use chrono::{DateTime, Utc};

use super::token_store::PersistedTokens;
use crate::connection::ConnectionRef;
use crate::handles::{AuthLeaseHandle, AuthLeaseSnapshot, DslTransitionError, LeaseKey};

pub fn persisted_token_expires_at_epoch_secs(tokens: &PersistedTokens) -> u64 {
    tokens
        .expires_at
        .map(|ts| ts.timestamp().max(0) as u64)
        .unwrap_or(u64::MAX)
}

pub fn publish_token_lifecycle_acquired(
    handle: &dyn AuthLeaseHandle,
    connection_ref: &ConnectionRef,
    tokens: &PersistedTokens,
) -> Result<(), DslTransitionError> {
    let lease_key = LeaseKey::from_connection_ref(connection_ref);
    handle.acquire_lease(&lease_key, persisted_token_expires_at_epoch_secs(tokens))
}

pub fn publish_token_lifecycle_released(
    handle: &dyn AuthLeaseHandle,
    connection_ref: &ConnectionRef,
) -> Result<(), DslTransitionError> {
    let lease_key = LeaseKey::from_connection_ref(connection_ref);
    handle.release_lease(&lease_key)
}

pub fn lease_snapshot_expires_at_datetime(snapshot: &AuthLeaseSnapshot) -> Option<DateTime<Utc>> {
    snapshot
        .expires_at
        .and_then(|secs| i64::try_from(secs).ok())
        .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::auth::PersistedAuthMode;
    use crate::connection::{BindingId, RealmId};
    use crate::handles::{AuthLeasePhase, DslTransitionError};

    #[derive(Default)]
    struct RecordingAuthLeaseHandle {
        acquired: Mutex<Vec<(LeaseKey, u64)>>,
        released: Mutex<Vec<LeaseKey>>,
    }

    impl RecordingAuthLeaseHandle {
        fn acquired(&self) -> Vec<(LeaseKey, u64)> {
            self.acquired
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl AuthLeaseHandle for RecordingAuthLeaseHandle {
        fn acquire_lease(
            &self,
            lease_key: &LeaseKey,
            expires_at: u64,
        ) -> Result<(), DslTransitionError> {
            self.acquired
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((lease_key.clone(), expires_at));
            Ok(())
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            self.released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(lease_key.clone());
            Ok(())
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: None,
            }
        }
    }

    fn connection_ref() -> ConnectionRef {
        ConnectionRef {
            realm: RealmId::parse("dev").expect("valid realm"),
            binding: BindingId::parse("default_openai").expect("valid binding"),
            profile: None,
        }
    }

    fn tokens_with_expiry(expires_at: Option<DateTime<Utc>>) -> PersistedTokens {
        PersistedTokens {
            auth_mode: PersistedAuthMode::ApiKey,
            primary_secret: Some("secret".into()),
            refresh_token: None,
            id_token: None,
            expires_at,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn lifecycle_acquire_uses_persisted_token_expiry_as_machine_input() {
        let handle = RecordingAuthLeaseHandle::default();
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let tokens = tokens_with_expiry(Some(expires_at));
        let connection_ref = connection_ref();

        publish_token_lifecycle_acquired(&handle, &connection_ref, &tokens).unwrap();

        assert_eq!(
            handle.acquired(),
            vec![(
                LeaseKey::from_connection_ref(&connection_ref),
                1_800_000_000
            )]
        );
    }

    #[test]
    fn lifecycle_acquire_maps_non_expiring_tokens_to_unbounded_lease() {
        let handle = RecordingAuthLeaseHandle::default();
        let tokens = tokens_with_expiry(None);

        publish_token_lifecycle_acquired(&handle, &connection_ref(), &tokens).unwrap();

        assert_eq!(handle.acquired()[0].1, u64::MAX);
    }
}
