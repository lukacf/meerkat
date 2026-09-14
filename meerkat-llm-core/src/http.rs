//! Shared HTTP client helpers.

use crate::error::LlmError;
use meerkat_core::execution_scope::{
    ScopedEffectOutcome, ScopedModelEffectCustody, ScopedModelRequest,
};
#[cfg(not(target_arch = "wasm32"))]
use std::net::IpAddr;
use std::sync::Arc;

pub struct ModelRequestSendEvidence<'a> {
    pub provider: meerkat_core::Provider,
    pub encoding: meerkat_core::LoweredRequestEncoding,
    pub model: &'a str,
    pub route: &'a str,
    pub body: &'a serde_json::Value,
    pub native_tools: meerkat_core::ProviderNativeToolPolicy,
}

/// The caller finishes authorization first. The physical send future is not
/// polled until this helper consumes its exact durable start permit.
pub async fn send_model_request<T>(
    scope: Option<Arc<ScopedModelRequest>>,
    evidence: ModelRequestSendEvidence<'_>,
    send: impl std::future::Future<Output = Result<T, LlmError>>,
) -> Result<(T, Option<ScopedModelEffectCustody>), LlmError> {
    let Some(scope) = scope else {
        return send.await.map(|response| (response, None));
    };
    let body = serde_json::to_vec(evidence.body).map_err(|error| LlmError::InvalidRequest {
        message: format!("model request encoding failed: {error}"),
    })?;
    let provenance = meerkat_core::LoweredRequestProvenance::from_body(
        evidence.provider,
        evidence.encoding,
        &body,
    );
    let mut custody = scope
        .claim_request(
            evidence.model,
            evidence.route,
            provenance,
            evidence.native_tools,
        )
        .await
        .map_err(scoped_model_error)?;
    custody.begin_invocation().map_err(scoped_model_error)?;
    match send.await {
        Ok(response) => Ok((response, Some(custody))),
        Err(error) => {
            custody
                .settle(ScopedEffectOutcome::Unknown)
                .await
                .map_err(|feedback| LlmError::Unknown {
                    message: format!(
                        "model send failed ({error}); scoped feedback failed: {feedback}"
                    ),
                })?;
            Err(error)
        }
    }
}

pub(crate) fn scoped_model_error(error: meerkat_core::ToolError) -> LlmError {
    LlmError::InvalidRequest {
        message: format!("scoped model effect refused: {error}"),
    }
}

#[allow(dead_code)]
pub fn build_http_client_for_base_url(
    builder: reqwest::ClientBuilder,
    base_url: &str,
) -> Result<reqwest::Client, LlmError> {
    // no_proxy is not available on wasm32 (browser handles proxies)
    #[cfg(not(target_arch = "wasm32"))]
    let builder = {
        let disable_proxy = cfg!(test) || is_loopback_base_url(base_url);
        if disable_proxy {
            builder.no_proxy()
        } else {
            builder
        }
    };
    #[cfg(target_arch = "wasm32")]
    let _ = base_url; // suppress unused warning

    builder.build().map_err(|e| LlmError::Unknown {
        message: format!("Failed to build HTTP client: {e}"),
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn is_loopback_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let normalized_host = host.trim_matches(&['[', ']'][..]);
    normalized_host.eq_ignore_ascii_case("localhost")
        || normalized_host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::is_loopback_base_url;

    #[test]
    fn test_is_loopback_base_url_localhost() {
        assert!(is_loopback_base_url("http://localhost:8080"));
    }

    #[test]
    fn test_is_loopback_base_url_ipv4() {
        assert!(is_loopback_base_url("http://127.0.0.1:8080"));
    }

    #[test]
    fn test_is_loopback_base_url_ipv6() {
        assert!(is_loopback_base_url("http://[::1]:8080"));
    }

    #[test]
    fn test_is_loopback_base_url_non_loopback() {
        assert!(!is_loopback_base_url("https://api.openai.com"));
    }
}
