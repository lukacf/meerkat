//! T9 — AWS SigV4 authorizer contract (Phase 4a).
//!
//! Choke-point: given a Bedrock request, `AwsStsAuthorizer::authorize`
//! adds an `Authorization: AWS4-HMAC-SHA256 ...` header with the correct
//! `Credential=<key>/<date>/<region>/<service>/aws4_request`,
//! `SignedHeaders=...`, and `Signature=<hex>`. Bearer-token mode emits a
//! plain `Authorization: Bearer <tok>` header instead.

#![cfg(all(not(target_arch = "wasm32"), feature = "aws-auth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_client::authorizers::{AwsCredentialProvider, AwsStsAuthorizer};
use meerkat_core::{HttpAuthorizationRequest, HttpAuthorizer};

fn static_provider() -> AwsCredentialProvider {
    AwsCredentialProvider::Static {
        access_key_id: "AKIATESTTESTTESTTEST".into(),
        secret_access_key: "secretsecretsecretsecretsecretsecretsecr".into(),
        session_token: Some("FQoGZXIvYXdzEJr//////////wEaDNTESTSESSIONTOKEN".into()),
    }
}

#[tokio::test]
async fn sigv4_adds_required_headers_for_bedrock() {
    let authorizer = AwsStsAuthorizer::new("us-east-1", static_provider());
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
    let authorizer = AwsStsAuthorizer::new("us-east-1", provider);
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
    let authorizer = AwsStsAuthorizer::new("us-west-2", provider);
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
