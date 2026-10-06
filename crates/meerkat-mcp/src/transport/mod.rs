pub(crate) mod protected;
pub(crate) mod sse;
pub(crate) mod streamable_http;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;

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
