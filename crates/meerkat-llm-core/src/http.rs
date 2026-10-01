//! Shared HTTP client helpers.

use crate::error::LlmError;
#[cfg(not(target_arch = "wasm32"))]
use std::net::IpAddr;

/// Execute the already-built immutable request at its actual HTTP boundary.
/// Auth/header awaits belong before this call. Entry failure prevents sending;
/// outcome failure appends one safe diagnostic and preserves the exact result.
/// The caller must forward diagnostics before handling the returned error, so
/// a transport failure cannot hide a failed audit observation. This local
/// output vector is not an audit journal, authority source, or retry state.
pub async fn execute_with_authorization(
    client: &reqwest::Client,
    request: reqwest::Request,
    check: Option<&meerkat_core::authorization::PreparedOperationCheck>,
    diagnostics: &mut Vec<crate::LlmEvent>,
    map_error: impl FnOnce(reqwest::Error) -> LlmError,
) -> Result<reqwest::Response, LlmError> {
    use meerkat_core::authorization::{OperationObservationPhase, OperationObservedOutcome};
    let current = check
        .map(|check| check.current())
        .transpose()
        .map_err(LlmError::from_operation_authorization)?;
    if let Some(check) = &current {
        check
            .observe_entry()
            .map_err(LlmError::from_operation_observation)?;
    }
    // Observation staging can synchronize with its owner. Recheck after it,
    // while retaining the exact refreshed decision for this physical send.
    let current = current
        .as_ref()
        .map(|check| check.current())
        .transpose()
        .map_err(LlmError::from_operation_authorization)?;
    let result = client.execute(request).await;
    if let Some(check) = current {
        let outcome = match &result {
            Ok(response) => OperationObservedOutcome::HttpResponse {
                status: response.status().as_u16(),
            },
            Err(_) => OperationObservedOutcome::TransportError,
        };
        if check.observe_outcome(outcome).is_err() {
            diagnostics.push(crate::LlmEvent::OperationObservationFailed {
                operation_id: check.binding().facts().operation_id.clone(),
                phase: OperationObservationPhase::Outcome,
            });
        }
    }
    result.map_err(map_error)
}

/// Validate configured text-route syntax before projecting nonsecret target
/// facts. Query parameters are added only by the concrete provider owner.
/// This supplies no permission and never rewrites the configured URL.
pub fn validate_authorization_base_url(base_url: &str) -> Result<(), LlmError> {
    let refused = || {
        LlmError::operation_refused(
            meerkat_core::authorization::OperationRefusalKind::MalformedFacts,
        )
    };
    let url = reqwest::Url::parse(base_url).map_err(|_| refused())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(refused());
    }
    Ok(())
}

/// Existing provider transport with redirect and opaque application retry
/// disabled. Explicit provider retries must re-enter their checked send helper.
/// Proxy selection and the provider's timeout/pool settings remain unchanged.
#[cfg(not(target_arch = "wasm32"))]
pub fn build_checked_http_client_for_base_url(
    builder: reqwest::ClientBuilder,
    base_url: &str,
) -> Result<reqwest::Client, LlmError> {
    build_http_client_for_base_url(
        builder
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never()),
        base_url,
    )
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
    use super::{is_loopback_base_url, validate_authorization_base_url};

    #[test]
    fn authorization_base_refuses_opaque_query_fragment_and_credentials() {
        for url in [
            "https://user:synthetic@example.invalid/api",
            "https://example.invalid/api?token=synthetic",
            "https://example.invalid/api?",
            "https://example.invalid/api#fragment",
            "file:///tmp/not-a-model",
        ] {
            assert!(matches!(
                validate_authorization_base_url(url),
                Err(crate::LlmError::OperationRefused { .. })
            ));
        }
        assert!(validate_authorization_base_url("https://example.invalid/declared/api").is_ok());
    }

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
