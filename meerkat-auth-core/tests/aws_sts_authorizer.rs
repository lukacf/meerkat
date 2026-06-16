//! T9 — AWS SigV4 authorizer contract (Phase 4a).
//!
//! Choke-point: given a Bedrock request, `AwsStsAuthorizer::authorize`
//! adds an `Authorization: AWS4-HMAC-SHA256 ...` header with the correct
//! `Credential=<key>/<date>/<region>/<service>/aws4_request`,
//! `SignedHeaders=...`, and `Signature=<hex>`. Bearer-token mode emits a
//! plain `Authorization: Bearer <tok>` header instead.

#![cfg(all(not(target_arch = "wasm32"), feature = "aws-sigv4"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_auth_core::authorizers::{AwsCredentialProvider, AwsStsAuthorizer};
use meerkat_core::handles::{AuthLeaseHandle, AuthLeasePhase, GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_core::{
    AuthError, BindingId, HttpAuthorizationRequest, HttpAuthorizer, ProfileId, RealmId,
};

fn generated_auth_lease_handle_for_test(
    handle: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
) -> GeneratedAuthLeaseHandle {
    meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(handle)
        .expect("runtime AuthLeaseHandle is certified by generated AuthMachine authority")
}

fn lease_key() -> LeaseKey {
    LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("bedrock").unwrap(),
        Some(ProfileId::parse("aws-sigv4").unwrap()),
    )
}

fn bedrock_request(headers: &mut Vec<(String, String)>) -> HttpAuthorizationRequest<'_> {
    HttpAuthorizationRequest {
        method: "POST",
        url: "https://bedrock-runtime.us-east-1.amazonaws.com/model/x/invoke",
        headers,
    }
}

fn static_provider() -> AwsCredentialProvider {
    AwsCredentialProvider::Static {
        access_key_id: "AKIATESTTESTTESTTEST".into(),
        secret_access_key: "secretsecretsecretsecretsecretsecretsecr".into(),
        session_token: Some("FQoGZXIvYXdzEJr//////////wEaDNTESTSESSIONTOKEN".into()),
    }
}

fn with_test_observer(authorizer: AwsStsAuthorizer) -> AwsStsAuthorizer {
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    authorizer.with_auth_lease_observer(generated_auth_lease_handle_for_test(handle), lease_key())
}

#[tokio::test]
async fn sigv4_adds_required_headers_for_bedrock() {
    let authorizer = with_test_observer(AwsStsAuthorizer::new("us-east-1", static_provider()));
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
async fn sigv4_without_auth_lease_observer_fails_closed_before_signing() {
    let authorizer = AwsStsAuthorizer::new("us-east-1", static_provider());
    let mut headers = vec![(
        "host".to_string(),
        "bedrock-runtime.us-east-1.amazonaws.com".to_string(),
    )];
    let mut req = bedrock_request(&mut headers);

    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, AuthError::HostOwnedUnavailable),
        "unobserved SigV4 signing must fail closed, got {err:?}"
    );
    assert!(
        !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization")),
        "unobserved SigV4 signing must not add authorization headers"
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
    let authorizer = with_test_observer(AwsStsAuthorizer::new("us-east-1", provider));
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
    let authorizer = with_test_observer(AwsStsAuthorizer::new("us-west-2", provider));
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

/// Row #49 gate: a wired AWS authorizer consults the per-binding AuthMachine
/// lease before signing. On first use the binding is absent, so the authorizer
/// acquires a `Valid` (no-expiry) lease for the Static credentials and signs —
/// the lease, not an implicit always-fresh assumption, now owns validity.
#[tokio::test]
async fn sigv4_acquires_valid_lease_on_first_use_then_signs() {
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let key = lease_key();
    let authorizer = AwsStsAuthorizer::new("us-east-1", static_provider())
        .with_auth_lease_observer(
            generated_auth_lease_handle_for_test(Arc::clone(&handle)),
            key.clone(),
        );

    // Absent before first use.
    assert_eq!(handle.snapshot(&key).phase, None);

    let mut headers = vec![(
        "host".to_string(),
        "bedrock-runtime.us-east-1.amazonaws.com".to_string(),
    )];
    let mut req = bedrock_request(&mut headers);
    authorizer.authorize(&mut req).await.unwrap();

    // The authorizer signed (proof the freshness verdict admitted use) ...
    assert!(headers.iter().any(
        |(k, v)| k.eq_ignore_ascii_case("authorization") && v.starts_with("AWS4-HMAC-SHA256 ")
    ));
    // ... and the AuthMachine lease — not a wall-clock assumption — now owns
    // the credential's validity, held as an explicit no-expiry `Valid` phase.
    assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Valid));
}

/// Row #49 gate: a reauth-required lease yields a typed `UserReauthRequired`
/// disposition before any SigV4 signing happens — no silent `SystemTime::now()`
/// sign with credential material the lease has rejected.
#[tokio::test]
async fn sigv4_reauth_required_lease_blocks_signing_with_typed_disposition() {
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let key = lease_key();
    handle.acquire_lease(&key, u64::MAX).unwrap();
    handle.mark_reauth_required(&key).unwrap();
    assert_eq!(
        handle.snapshot(&key).phase,
        Some(AuthLeasePhase::ReauthRequired)
    );

    let authorizer = AwsStsAuthorizer::new("us-east-1", static_provider())
        .with_auth_lease_observer(
            generated_auth_lease_handle_for_test(Arc::clone(&handle)),
            key,
        );

    let mut headers = vec![(
        "host".to_string(),
        "bedrock-runtime.us-east-1.amazonaws.com".to_string(),
    )];
    let mut req = bedrock_request(&mut headers);
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, AuthError::UserReauthRequired),
        "reauth-required lease must surface UserReauthRequired, got {err:?}"
    );
    // Fail closed: nothing was signed.
    assert!(
        !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    );
}

/// Row #49 gate: an expired credential lease (STS session token past its bound
/// expiry) refuses to sign and surfaces a typed `RefreshRequired` rather than
/// silently re-signing with the stale credential.
#[tokio::test]
async fn sigv4_expired_lease_blocks_signing_with_typed_disposition() {
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let key = lease_key();
    // Acquire with a near-past expiry, then observe freshness in the future so
    // the AuthMachine moves the lease to Expired.
    handle.acquire_lease(&key, 1_000).unwrap();
    handle
        .observe_credential_freshness(&key, 1_000_000, 60)
        .unwrap();
    assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Expired));

    let authorizer = AwsStsAuthorizer::new("us-east-1", static_provider())
        .with_auth_lease_observer(
            generated_auth_lease_handle_for_test(Arc::clone(&handle)),
            key,
        );

    let mut headers = vec![(
        "host".to_string(),
        "bedrock-runtime.us-east-1.amazonaws.com".to_string(),
    )];
    let mut req = bedrock_request(&mut headers);
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, AuthError::RefreshRequired),
        "expired lease must surface RefreshRequired, got {err:?}"
    );
    assert!(
        !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    );
}
