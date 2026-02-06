//! Shared HTTP client helpers.

use crate::error::LlmError;

pub fn build_http_client(builder: reqwest::ClientBuilder) -> Result<reqwest::Client, LlmError> {
    let builder = if cfg!(test) {
        builder.no_proxy()
    } else {
        builder
    };

    builder.build().map_err(|e| LlmError::Unknown {
        message: format!("Failed to build HTTP client: {}", e),
    })
}
