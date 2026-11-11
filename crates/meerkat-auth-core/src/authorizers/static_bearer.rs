//! Static bearer-token authorizer.
//!
//! Used by auth paths that already own a concrete bearer token string
//! and need to inject it as `Authorization: Bearer <token>` on every
//! request — notably Anthropic's Bedrock backend when the caller
//! supplies `AWS_BEARER_TOKEN_BEDROCK` directly (plan §Phase 4b.6
//! `bedrock_bearer` method) rather than STS-signing via SigV4.
//!
//! This is a trivial wrapper over [`HttpAuthorizer`] — no refresh, no
//! network — but it lets the provider client use the same Bearer-mode
//! code path as OAuth/ADC authorizers, avoiding a parallel flat-path
//! branch inside provider `build_client` implementations.

use async_trait::async_trait;

use meerkat_core::{AuthError, HttpAuthorizationRequest, HttpAuthorizer};

/// Authorizer that unconditionally sets `Authorization: Bearer <token>`.
///
/// Instantiate with [`StaticBearerAuthorizer::new`]. The token is
/// captured at construction time and cannot rotate — callers who need
/// refresh must construct a new authorizer.
pub struct StaticBearerAuthorizer {
    token: String,
    label: &'static str,
}

impl StaticBearerAuthorizer {
    /// Construct a bearer authorizer with the given static token and
    /// a label used for diagnostics.
    pub fn new(token: String, label: &'static str) -> Self {
        Self { token, label }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl HttpAuthorizer for StaticBearerAuthorizer {
    async fn authorize(&self, req: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError> {
        req.headers.push((
            "Authorization".to_string(),
            format!("Bearer {}", self.token),
        ));
        Ok(())
    }

    fn visit_signed_credentials(&self, visit: &mut dyn FnMut(&str)) {
        visit(&self.token);
    }

    fn label(&self) -> &str {
        self.label
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sets_bearer_header() {
        let authorizer = StaticBearerAuthorizer::new("tok-123".to_string(), "bedrock-bearer");
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://bedrock-runtime.us-east-1.amazonaws.com/model/claude/invoke",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "Authorization");
        assert_eq!(headers[0].1, "Bearer tok-123");
        assert_eq!(authorizer.label(), "bedrock-bearer");
    }

    /// What a provider, proxy or gateway that repeats a request can send
    /// back: each header the authorizer added, as sent, in a header line and
    /// alone; the bare credential; a URL carrying it.
    fn echoes(headers: &[(String, String)], credential: &str) -> Vec<String> {
        let mut echoes = vec![
            format!("upstream echo: credential {credential} was refused"),
            format!(
                "upstream echo: GET http://upstream.invalid/v1/models?session={credential}&page=2"
            ),
        ];
        for (name, value) in headers {
            echoes.push(format!(
                r#"{{"error":{{"message":"upstream echo: rejected request with header {name}: {value}","type":"invalid_request_error"}}}}"#
            ));
            echoes.push(format!("upstream echo: value {value} was refused"));
        }
        echoes
    }

    /// Sign one request with `authorizer`, then run every echo of it
    /// through the authorizer's redaction: none of `credentials` is left,
    /// and the text around them is.
    async fn assert_redacts_what_it_signed(authorizer: &dyn HttpAuthorizer, credentials: &[&str]) {
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "http://upstream.invalid/v1/chat/completions",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
        assert!(!headers.is_empty(), "the authorizer signs the request");
        for credential in credentials {
            for echo in echoes(&headers, credential) {
                let mut diagnostic = echo;
                authorizer.redact_response_diagnostic(&mut diagnostic);
                for credential in credentials {
                    assert!(
                        !diagnostic.contains(credential),
                        "{} left a credential in: {diagnostic}",
                        authorizer.label()
                    );
                }
                assert!(
                    diagnostic.contains("upstream echo"),
                    "the text around the credential is kept: {diagnostic}"
                );
            }
        }
    }

    /// A response that repeats the request's bearer, in a header line, alone
    /// or in a URL, loses it in the authorizer's redaction.
    #[tokio::test]
    async fn redacts_the_bearer_it_signed_with() {
        let token = format!("static-{}", "madeupbearer".repeat(3));
        let authorizer = StaticBearerAuthorizer::new(token.clone(), "static-bearer");
        assert_redacts_what_it_signed(&authorizer, &[&token]).await;
    }

    /// Static headers from an external resolver: a bearer, a Basic pair and
    /// a key header are each redacted as sent and, for the two with a
    /// scheme, without it.
    #[tokio::test]
    async fn static_headers_redact_every_value_they_sign_with() {
        let bearer = format!("resolver-{}", "madeupbearer".repeat(3));
        let basic = format!("{}==", "bWFkZXVwOnBhaXI".repeat(2));
        let key = format!("resolver-{}", "madeupkey".repeat(3));
        let authorizer = crate::resolver::StaticHeadersAuthorizer::new(
            vec![
                ("Authorization".to_string(), format!("Bearer {bearer}")),
                ("Proxy-Authorization".to_string(), format!("Basic {basic}")),
                ("x-api-key".to_string(), key.clone()),
            ],
            "external:static_headers",
        );
        assert_redacts_what_it_signed(&authorizer, &[&bearer, &basic, &key]).await;
    }

    /// Static headers that carry no credential (a model id, a content
    /// type, a deployment name) are not named: an error that mentions
    /// their values stays readable. The value of a header whose name says
    /// it is a secret is removed, whatever the case of the name.
    #[tokio::test]
    async fn static_headers_name_only_the_values_of_credential_headers() {
        let key = format!("resolver-{}", "madeupkey".repeat(3));
        let authorizer = crate::resolver::StaticHeadersAuthorizer::new(
            vec![
                ("x-model".to_string(), "gpt-5.4-mini".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
                ("x-deployment".to_string(), "production-east".to_string()),
                ("x-acme-api-key".to_string(), key.clone()),
            ],
            "external:static_headers",
        );
        let mut diagnostic = format!(
            "The model `gpt-5.4-mini` does not exist; content type application/json refused \
             for deployment production-east (value {key})"
        );
        authorizer.redact_response_diagnostic(&mut diagnostic);
        assert_eq!(
            diagnostic,
            "The model `gpt-5.4-mini` does not exist; content type application/json refused \
             for deployment production-east (value [REDACTED])"
        );

        for name in [
            "Authorization",
            "Proxy-Authorization",
            "Cookie",
            "X-Acme-API-Key",
            "x-upstream-token",
            "x-client-secret",
            "x-upstream-auth",
            "x-store-password",
            "x-credential",
            "x-request-signature",
            "x-session-id",
        ] {
            let value = format!("resolver-{}", "madeupvalue".repeat(3));
            let authorizer = crate::resolver::StaticHeadersAuthorizer::new(
                vec![(name.to_string(), value.clone())],
                "external:static_headers",
            );
            let mut diagnostic = format!("upstream echo: value {value} was refused");
            authorizer.redact_response_diagnostic(&mut diagnostic);
            assert_eq!(
                diagnostic, "upstream echo: value [REDACTED] was refused",
                "{name}"
            );
        }
    }

    /// Text that carries no credential is returned as it was, short words
    /// and a short header value included: a value too short to be a
    /// credential is never replaced.
    #[tokio::test]
    async fn redaction_leaves_text_without_a_credential_unchanged() {
        let token = format!("static-{}", "madeupbearer".repeat(3));
        let bearer = StaticBearerAuthorizer::new(token, "static-bearer");
        let headers = crate::resolver::StaticHeadersAuthorizer::new(
            vec![("x-tenant".to_string(), "acme".to_string())],
            "external:static_headers",
        );
        for authorizer in [&bearer as &dyn HttpAuthorizer, &headers] {
            for text in [
                "model none is not served by tenant acme; basic plan, key rotation due\n",
                // Ordinary provider errors that only talk about credentials,
                // with a type, a code, a request id and a URL.
                r#"{"error":{"message":"You didn't provide an API key. You need to provide your API key in an Authorization header using Bearer auth (i.e. Authorization: Bearer YOUR_KEY), or as the password field (with blank username) if you're accessing the API from your browser. You can obtain an API key from https://docs.upstream.invalid/account/api-keys.","type":"invalid_request_error","param":null,"code":null}}"#,
                r#"{"error":{"message":"Incorrect API key provided: sk-proj-********************abcd. You can find your API key at https://docs.upstream.invalid/account/api-keys.","type":"invalid_request_error","param":null,"code":"invalid_api_key"},"request_id":"req_0123456789abcdef"}"#,
                r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},"request_id":"req_011CSHoEeqs5C35K2UUqR7Fy"}"#,
                "{\n  \"error\": {\n    \"code\": 400,\n    \"message\": \"API key not valid. Please pass a valid API key.\",\n    \"status\": \"INVALID_ARGUMENT\",\n    \"details\": [\n      {\n        \"@type\": \"type.googleapis.com/google.rpc.ErrorInfo\",\n        \"reason\": \"API_KEY_INVALID\",\n        \"domain\": \"googleapis.com\"\n      }\n    ]\n  }\n}\n",
                r#"{"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 130112 tokens. Please reduce the length of the messages.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#,
                "Missing bearer authentication in header; basic authentication is not supported. See https://docs.upstream.invalid/v1/auth?section=2#tokens",
            ] {
                let mut diagnostic = text.to_string();
                authorizer.redact_response_diagnostic(&mut diagnostic);
                assert_eq!(diagnostic, text, "{}", authorizer.label());
            }
        }
    }
}
