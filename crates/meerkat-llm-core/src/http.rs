//! Shared HTTP client helpers.

use crate::error::LlmError;
#[cfg(not(target_arch = "wasm32"))]
use std::net::IpAddr;

/// Build a provider HTTP client for `base_url`.
///
/// Native clients never follow redirects, same-origin included: a request
/// goes only to the endpoint it was built for, and a 3xx answer comes back
/// to the provider as the configured endpoint's response. On wasm32 the
/// browser owns redirect handling.
#[allow(dead_code)]
pub fn build_http_client_for_base_url(
    builder: reqwest::ClientBuilder,
    base_url: &str,
) -> Result<reqwest::Client, LlmError> {
    #[cfg(not(target_arch = "wasm32"))]
    let builder = builder.redirect(reqwest::redirect::Policy::none());
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
#[allow(clippy::expect_used)]
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

    /// Accept one connection on `listener`, read the request head, and answer
    /// with `response`.
    #[cfg(not(target_arch = "wasm32"))]
    async fn answer_once(listener: tokio::net::TcpListener, response: String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut head = Vec::new();
        let mut buf = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
    }

    /// Every provider client builds its HTTP client here, so a redirect from
    /// the configured endpoint to another host is never followed: the 3xx
    /// comes back to the provider instead of a request to the other host.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn provider_http_client_does_not_follow_a_cross_host_redirect() {
        let target = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind target");
        let target_addr = target.local_addr().expect("target addr");
        let origin = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind origin");
        let origin_addr = origin.local_addr().expect("origin addr");
        let origin_server = tokio::spawn(answer_once(
            origin,
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/v1/messages\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        ));
        let reached_target = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reached = std::sync::Arc::clone(&reached_target);
        let target_server = tokio::spawn(async move {
            if target.accept().await.is_ok() {
                reached.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });

        let base_url = format!("http://{origin_addr}");
        let client = super::build_http_client_for_base_url(reqwest::Client::builder(), &base_url)
            .expect("client");
        let response = client
            .post(format!("{base_url}/v1/messages"))
            .header("x-api-key", "sk-redirect-probe")
            .body("{}")
            .send()
            .await;
        // A followed redirect only completes `send` after the target accepted
        // the follow-up connection, so the flag is settled here.
        let followed = reached_target.load(std::sync::atomic::Ordering::SeqCst);
        origin_server.await.expect("origin served");
        target_server.abort();
        assert!(
            !followed,
            "the provider HTTP client followed a redirect to another host"
        );
        let response = response.expect("the configured endpoint answers");
        assert_eq!(response.status().as_u16(), 302);
    }
}
