//! Shared HTTP client helpers.

use crate::error::LlmError;
use std::net::IpAddr;

pub fn build_http_client_for_base_url(
    builder: reqwest::ClientBuilder,
    base_url: &str,
) -> Result<reqwest::Client, LlmError> {
    let disable_proxy = cfg!(test) || is_loopback_base_url(base_url);
    let builder = if disable_proxy {
        builder.no_proxy()
    } else {
        builder
    };

    builder.build().map_err(|e| LlmError::Unknown {
        message: format!("Failed to build HTTP client: {}", e),
    })
}

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
