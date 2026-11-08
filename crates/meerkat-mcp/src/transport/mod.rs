pub(crate) mod abandoned;
pub(crate) mod protected;
pub(crate) mod sse;
pub(crate) mod streamable_http;

use meerkat_auth_core::http_egress::{
    HttpEgressError, HttpEgressProfile, HttpEgressUnavailable, egress_refusal_in,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// The host egress profile of one connection attempt, applied by its
/// transport client at every request, with the attempt's first egress
/// outcome recorded so the attempt fails typed rather than as transport text.
#[derive(Clone, Debug, Default)]
pub(crate) struct EgressGuard {
    profile: HttpEgressProfile,
    recorded: Arc<OnceLock<HttpEgressError>>,
}

impl EgressGuard {
    pub(crate) fn new(profile: HttpEgressProfile) -> Self {
        Self {
            profile,
            recorded: Arc::default(),
        }
    }

    /// The first egress refusal or unavailability this attempt met.
    pub(crate) fn recorded(&self) -> Option<HttpEgressError> {
        self.recorded.get().copied()
    }

    /// Record this attempt's egress outcome; the first one is kept.
    pub(crate) fn record_outcome(&self, error: HttpEgressError) -> HttpEgressError {
        let _ = self.recorded.set(error);
        error
    }

    /// Admit one request URI, recording a refusal. Nothing is sent when
    /// this fails.
    pub(crate) fn admit(&self, uri: &str) -> Result<(), HttpEgressError> {
        self.profile
            .admit(uri)
            .map_err(|error| self.record_outcome(error))
    }

    /// Record a request this profile did not admit: here (connection
    /// diagnostics) and, for a tool call, on the request's own disposition,
    /// which alone types that call's outcome. Nothing was sent for it.
    pub(crate) fn refuse(
        &self,
        dispatch: Option<&streamable_http::RequestDispatch>,
        error: HttpEgressError,
    ) -> HttpEgressError {
        self.record_outcome(error);
        if let Some(dispatch) = dispatch {
            dispatch.record(streamable_http::RequestDisposition::RefusedEgress(error));
        }
        error
    }

    /// The required ordinary client; `None` keeps the caller's existing
    /// trusted-host client.
    pub(crate) fn ordinary_client(
        &self,
    ) -> Option<Result<&reqwest::Client, HttpEgressUnavailable>> {
        self.profile.ordinary_client()
    }

    /// The required protected-call client; `None` keeps the existing one.
    pub(crate) fn protected_client(
        &self,
    ) -> Option<Result<&reqwest::Client, HttpEgressUnavailable>> {
        self.profile.protected_client()
    }

    /// The egress refusal a failed send carries (the clients' own resolver
    /// refused the destination, so nothing was connected), recorded.
    pub(crate) fn observe_send_error(
        &self,
        error: &reqwest::Error,
    ) -> Option<meerkat_auth_core::http_egress::HttpEgressRefusal> {
        let refusal = egress_refusal_in(error)?;
        self.record_outcome(HttpEgressError::Refused(refusal));
        Some(refusal)
    }
}

pub(crate) fn headers_from_map(headers: &HashMap<String, String>) -> Result<HeaderMap, String> {
    let mut header_map = HeaderMap::new();
    for (key, value) in headers {
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|e| format!("Invalid header name '{key}': {e}"))?;
        let value = HeaderValue::from_str(value)
            .map_err(|e| format!("Invalid header value for '{key}': {e}"))?;
        header_map.insert(name, value);
    }
    Ok(header_map)
}

/// Debug view of configured request headers: names only, values redacted.
/// Header values are host configuration and commonly carry credentials.
pub(crate) struct RedactedHeaders<'a>(pub(crate) &'a HeaderMap);

impl std::fmt::Debug for RedactedHeaders<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&str> = self.0.keys().map(HeaderName::as_str).collect();
        names.sort_unstable();
        f.debug_map()
            .entries(names.into_iter().map(|name| (name, "<redacted>")))
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn transport_clients_debug_redacts_header_values() {
        const SECRET: &str = "sk-live-secret-value";
        let headers = headers_from_map(&HashMap::from([
            ("Authorization".to_string(), format!("Bearer {SECRET}")),
            ("X-Api-Key".to_string(), SECRET.to_string()),
        ]))
        .expect("valid headers");
        let sse = format!("{:?}", sse::ReqwestSseClient::new(headers.clone()));
        let streamable = format!(
            "{:#?}",
            streamable_http::ReqwestStreamableHttpClient::new_with_auth_challenge(
                headers,
                streamable_http::AuthChallengeRecorder::default(),
            )
        );
        for rendered in [sse, streamable] {
            assert!(!rendered.contains(SECRET), "secret leaked: {rendered}");
            assert!(rendered.contains("authorization"), "{rendered}");
            assert!(rendered.contains("x-api-key"), "{rendered}");
            assert!(rendered.contains("<redacted>"), "{rendered}");
        }
    }
}
