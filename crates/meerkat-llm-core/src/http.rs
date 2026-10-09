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
    // A model request carries no operation review: its current decision may
    // enter only at R1, before any entry observation or send.
    let unreviewed = |check: &meerkat_core::authorization::PreparedOperationCheck| {
        check
            .require_unreviewed_entry()
            .map_err(|refusal| LlmError::OperationReviewRefused { refusal })
    };
    if let Some(check) = &current {
        unreviewed(check)?;
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
    if let Some(check) = &current {
        unreviewed(check)?;
    }
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

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod review_entry_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat_core::authorization::{
        OperationObservation, OperationObservationError, PreparedOperationCheck,
    };
    use meerkat_core::{
        AuthorizationOperation, MemorySearchScope, OperationAuthorizationError,
        OperationAuthorizationFacts, OperationExecutionScope, OperationId, OperationReviewRefusal,
        OperationReviewTier, PreparedAuthorizationBinding, PreparedOperationAuthorization,
        ReviewUnavailableKind, ReviewUnsatisfiedKind, SessionId, SourceAuthorizationFacts,
        SourceAuthorizationTarget, SourceAuthorizationUse, WorkAuthorization,
        WorkAuthorizationContext,
    };

    use crate::LlmError;

    struct Tiered {
        tier: OperationReviewTier,
        entries: Arc<AtomicUsize>,
    }

    impl PreparedOperationAuthorization for Tiered {
        fn review_tier(&self) -> OperationReviewTier {
            self.tier
        }

        fn check_current(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            Ok(())
        }

        fn observe(
            &self,
            _: &PreparedAuthorizationBinding,
            observation: OperationObservation,
        ) -> Result<(), OperationObservationError> {
            if matches!(observation, OperationObservation::Entry) {
                self.entries.fetch_add(1, Ordering::AcqRel);
            }
            Ok(())
        }
    }

    struct Owner(OperationReviewTier, Arc<AtomicUsize>);

    impl WorkAuthorization for Owner {
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(Tiered {
                tier: self.0,
                entries: Arc::clone(&self.1),
            }))
        }
    }

    fn check(tier: OperationReviewTier, entries: &Arc<AtomicUsize>) -> PreparedOperationCheck {
        PreparedOperationCheck::prepare(
            WorkAuthorizationContext::new(
                Arc::new(Owner(tier, Arc::clone(entries))),
                OperationExecutionScope::Domain,
            ),
            PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
                operation_id: OperationId::new(),
                execution_scope: OperationExecutionScope::Domain,
                run_id: None,
                context_revision: None,
                operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                    target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                        SessionId::new(),
                    )),
                    usage: SourceAuthorizationUse::Read,
                }),
            }),
        )
        .unwrap()
    }

    /// A loopback provider that counts every accepted connection and drops
    /// it, so an entered request fails at the transport after arriving.
    async fn counting_provider() -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&accepted);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::AcqRel);
                drop(socket);
            }
        });
        (format!("http://127.0.0.1:{port}/"), accepted)
    }

    #[tokio::test]
    async fn model_entry_refuses_required_review_before_observation_or_send() {
        let client = reqwest::Client::new();
        for (tier, expected) in [
            (OperationReviewTier::R1, None),
            (
                OperationReviewTier::R2,
                Some(OperationReviewRefusal::Unavailable {
                    kind: ReviewUnavailableKind::UnsupportedEntry,
                }),
            ),
            (
                OperationReviewTier::R3,
                Some(OperationReviewRefusal::Unsatisfied {
                    kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                }),
            ),
        ] {
            let entries = Arc::new(AtomicUsize::new(0));
            let check = check(tier, &entries);
            let (endpoint, accepted) = counting_provider().await;
            let request = client.get(endpoint).build().unwrap();
            let sent = Arc::new(AtomicUsize::new(0));
            let mut diagnostics = Vec::new();
            let result = super::execute_with_authorization(
                &client,
                request,
                Some(&check),
                &mut diagnostics,
                |_| {
                    sent.fetch_add(1, Ordering::AcqRel);
                    LlmError::ConnectionReset
                },
            )
            .await;
            match expected {
                // Positive control: R1 enters and reaches the transport.
                None => {
                    assert!(matches!(result, Err(LlmError::ConnectionReset)));
                    assert_eq!(entries.load(Ordering::Acquire), 1);
                    assert_eq!(sent.load(Ordering::Acquire), 1);
                    assert!(accepted.load(Ordering::Acquire) >= 1, "the request arrived");
                }
                Some(refusal) => {
                    assert!(
                        matches!(&result, Err(LlmError::OperationReviewRefused { refusal: actual }) if *actual == refusal),
                        "{tier:?}"
                    );
                    let error = result.unwrap_err();
                    assert!(!error.is_retryable());
                    assert_eq!(entries.load(Ordering::Acquire), 0);
                    assert_eq!(sent.load(Ordering::Acquire), 0);
                    tokio::task::yield_now().await;
                    assert_eq!(
                        accepted.load(Ordering::Acquire),
                        0,
                        "zero provider requests"
                    );
                }
            }
            assert!(diagnostics.is_empty());
        }
    }

    /// The model-request review refusal keeps its type through the agent
    /// error and stays in the authority-unavailable class (never a
    /// permission refusal). This pins the conversion only; it does not prove
    /// agent continuation.
    #[test]
    fn review_refusal_keeps_its_type_in_the_authority_unavailable_class() {
        let refusal = OperationReviewRefusal::Unsatisfied {
            kind: ReviewUnsatisfiedKind::HumanConsentRequired,
        };
        let error = LlmError::OperationReviewRefused { refusal };
        assert!(!error.is_retryable());
        let agent = error.into_agent_error("fixture");
        assert_eq!(agent.operation_review_refusal(), Some(refusal));
        assert!(agent.operation_authorization_unavailable());
        assert!(
            agent.operation_refusal().is_none(),
            "not a permission refusal"
        );
    }
}
