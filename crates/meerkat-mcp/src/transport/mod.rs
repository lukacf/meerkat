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
