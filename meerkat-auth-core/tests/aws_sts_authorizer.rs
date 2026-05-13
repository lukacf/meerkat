//! T9 — AWS SigV4 authorizer contract (Phase 4a).
//!
//! Choke-point: given a Bedrock request, `AwsStsAuthorizer::authorize`
//! adds an `Authorization: AWS4-HMAC-SHA256 ...` header with the correct
//! `Credential=<key>/<date>/<region>/<service>/aws4_request`,
//! `SignedHeaders=...`, and `Signature=<hex>`. Bearer-token mode emits a
//! plain `Authorization: Bearer <tok>` header instead.

#![cfg(all(not(target_arch = "wasm32"), feature = "aws-sigv4"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use meerkat_auth_core::authorizers::{AwsCredentialProvider, AwsStsAuthorizer};
use meerkat_core::connection::{BindingId, ProfileId, RealmId};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition, DslTransitionError,
    LeaseKey,
};
use meerkat_core::{HttpAuthorizationRequest, HttpAuthorizer};

fn static_provider() -> AwsCredentialProvider {
    AwsCredentialProvider::Static {
        access_key_id: "AKIATESTTESTTESTTEST".into(),
        secret_access_key: "secretsecretsecretsecretsecretsecretsecr".into(),
        session_token: Some("FQoGZXIvYXdzEJr//////////wEaDNTESTSESSIONTOKEN".into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LeaseEvent {
    Acquire(LeaseKey, u64),
    BeginRefresh(LeaseKey),
    CompleteRefresh(LeaseKey, u64),
    RefreshFailed(LeaseKey, bool),
    Snapshot,
}

struct RecordingAuthLeaseHandle {
    snapshot: Mutex<AuthLeaseSnapshot>,
    generation: Mutex<u64>,
    events: Mutex<Vec<LeaseEvent>>,
}

impl Default for RecordingAuthLeaseHandle {
    fn default() -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                credential_present: false,
                generation: 0,
                credential_published_at_millis: None,
            }),
            generation: Mutex::new(0),
            events: Mutex::new(Vec::new()),
        }
    }
}

impl RecordingAuthLeaseHandle {
    fn events(&self) -> Vec<LeaseEvent> {
        self.events.lock().unwrap().clone()
    }

    fn accept_valid_transition(
        &self,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let accepted_generation = {
            let mut generation = self.generation.lock().unwrap();
            *generation += 1;
            *generation
        };
        *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: (expires_at != u64::MAX).then_some(expires_at),
            credential_present: true,
            generation: accepted_generation,
            credential_published_at_millis: Some(10_000 + accepted_generation),
        };
        Ok(AuthLeaseTransition {
            generation: accepted_generation,
            credential_published_at_millis: Some(10_000 + accepted_generation),
        })
    }
}

impl AuthLeaseHandle for RecordingAuthLeaseHandle {
    fn acquire_lease(
        &self,
        lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::Acquire(lease_key.clone(), expires_at));
        self.accept_valid_transition(expires_at)
    }

    fn mark_expiring(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = Some(AuthLeasePhase::Expiring);
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::BeginRefresh(lease_key.clone()));
        Ok(())
    }

    fn begin_refresh(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::BeginRefresh(lease_key.clone()));
        self.snapshot.lock().unwrap().phase = Some(AuthLeasePhase::Refreshing);
        Ok(())
    }

    fn complete_refresh(
        &self,
        lease_key: &LeaseKey,
        new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::CompleteRefresh(
                lease_key.clone(),
                new_expires_at,
            ));
        self.accept_valid_transition(new_expires_at)
    }

    fn refresh_failed(
        &self,
        lease_key: &LeaseKey,
        permanent: bool,
    ) -> Result<(), DslTransitionError> {
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::RefreshFailed(lease_key.clone(), permanent));
        Ok(())
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.snapshot.lock().unwrap().phase = Some(AuthLeasePhase::ReauthRequired);
        Ok(())
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Released),
            expires_at: None,
            credential_present: false,
            generation: *self.generation.lock().unwrap(),
            credential_published_at_millis: None,
        };
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.events.lock().unwrap().push(LeaseEvent::Snapshot);
        self.snapshot.lock().unwrap().clone()
    }
}

fn lease_key() -> LeaseKey {
    LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("bedrock").unwrap(),
        Some(ProfileId::parse("aws_sigv4").unwrap()),
    )
}

fn with_recording_auth_lease(
    authorizer: AwsStsAuthorizer,
) -> (AwsStsAuthorizer, Arc<RecordingAuthLeaseHandle>, LeaseKey) {
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = lease_key();
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    (
        authorizer.with_auth_lease_observer(lease_handle, lease_key.clone()),
        handle,
        lease_key,
    )
}

fn fixed_signing_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

#[tokio::test]
async fn sigv4_adds_required_headers_for_bedrock() {
    let (authorizer, _, _) = with_recording_auth_lease(
        AwsStsAuthorizer::new("us-east-1", static_provider())
            .with_signing_time_source(fixed_signing_time),
    );
    let mut headers = vec![(
        "host".to_string(),
        "bedrock-runtime.us-east-1.amazonaws.com".to_string(),
    )];
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-3-5-sonnet-20241022-v2%3A0/invoke",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .expect("Authorization header must be set");
    assert!(
        auth.1.starts_with("AWS4-HMAC-SHA256 "),
        "expected SigV4 prefix, got {}",
        auth.1
    );
    assert!(auth.1.contains("Credential=AKIATESTTESTTESTTEST/"));
    assert!(auth.1.contains("/us-east-1/bedrock/aws4_request"));
    assert!(auth.1.contains("SignedHeaders="));
    assert!(auth.1.contains("Signature="));

    // Session token surfaces as x-amz-security-token.
    assert!(
        headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-amz-security-token") && v.starts_with("FQoG"))
    );
    // X-Amz-Date is required for SigV4.
    assert!(
        headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("x-amz-date"))
    );
}

#[tokio::test]
async fn sigv4_signing_publishes_and_reuses_authmachine_lease() {
    let (authorizer, handle, lease_key) = with_recording_auth_lease(
        AwsStsAuthorizer::new("us-east-1", static_provider())
            .with_signing_time_source(fixed_signing_time),
    );

    for _ in 0..2 {
        let mut headers = vec![(
            "host".into(),
            "bedrock-runtime.us-east-1.amazonaws.com".into(),
        )];
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://bedrock-runtime.us-east-1.amazonaws.com/model/x/invoke",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
    }

    let events = handle.events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LeaseEvent::Acquire(_, _)))
            .count(),
        1,
        "first SigV4 authorization should publish one long-lived AuthMachine lease"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LeaseEvent::Acquire(key, u64::MAX) if key == &lease_key)),
        "long-lived AWS credentials should be represented by the AuthMachine lease owner"
    );
    assert!(
        events
            .iter()
            .filter(|event| matches!(event, LeaseEvent::Snapshot))
            .count()
            >= 2,
        "every signature should check AuthMachine freshness before signing"
    );
}

#[tokio::test]
async fn sigv4_requires_authmachine_lease_observer() {
    let authorizer = AwsStsAuthorizer::new("us-east-1", static_provider());
    let mut headers = vec![(
        "host".into(),
        "bedrock-runtime.us-east-1.amazonaws.com".into(),
    )];
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://bedrock-runtime.us-east-1.amazonaws.com/model/x/invoke",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::AuthError::HostOwnedUnavailable),
        "expected HostOwnedUnavailable, got {err:?}"
    );
}

#[tokio::test]
async fn bearer_token_mode_emits_bearer_header_and_skips_sigv4() {
    let authorizer = AwsStsAuthorizer::new(
        "us-east-1",
        AwsCredentialProvider::BearerToken("bedrock-bearer-xyz".into()),
    );
    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://bedrock-runtime.us-east-1.amazonaws.com/",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].0, "Authorization");
    assert_eq!(headers[0].1, "Bearer bedrock-bearer-xyz");
    // No SigV4 noise.
    assert!(!headers[0].1.contains("AWS4-HMAC-SHA256"));
}

#[tokio::test]
async fn env_provider_missing_keys_returns_missing_secret() {
    let provider = AwsCredentialProvider::from_env(|_| None);
    let (authorizer, _, _) =
        with_recording_auth_lease(AwsStsAuthorizer::new("us-east-1", provider));
    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://bedrock-runtime.us-east-1.amazonaws.com/",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::AuthError::MissingSecret),
        "expected MissingSecret, got {err:?}",
    );
}

#[tokio::test]
async fn env_provider_reads_sigv4_credentials_from_env_lookup() {
    let provider = AwsCredentialProvider::from_env(|k| match k {
        "AWS_ACCESS_KEY_ID" => Some("AKIAFROMENV00000000".into()),
        "AWS_SECRET_ACCESS_KEY" => Some("secretfromenvsecretfromenvsecretfromenv".into()),
        _ => None,
    });
    let (authorizer, _, _) = with_recording_auth_lease(
        AwsStsAuthorizer::new("us-west-2", provider).with_signing_time_source(fixed_signing_time),
    );
    let mut headers = vec![(
        "host".into(),
        "bedrock-runtime.us-west-2.amazonaws.com".into(),
    )];
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://bedrock-runtime.us-west-2.amazonaws.com/model/x/invoke",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();
    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .unwrap();
    assert!(auth.1.contains("Credential=AKIAFROMENV00000000/"));
    assert!(auth.1.contains("/us-west-2/bedrock/aws4_request"));
}

#[tokio::test]
async fn label_is_stable() {
    let a = AwsStsAuthorizer::new("eu-west-1", static_provider());
    assert_eq!(a.label(), "aws-sigv4(bedrock/eu-west-1)");

    let b = AwsStsAuthorizer::with_service("eu-west-1", "lambda", static_provider());
    assert_eq!(b.label(), "aws-sigv4(lambda/eu-west-1)");
}
