//! Typed substrate for the Meerkat-owned web-search fallback tool.
//!
//! Provider-native search remains provider-owned. These types describe the
//! optional Meerkat tool that can delegate a search-capable provider call when
//! the active model lacks native search support, such as live realtime models.

use crate::Provider;
use crate::lifecycle::run_primitive::ModelId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WEB_SEARCH_TOOL_NAME: &str = "web_search";

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchStatus {
    Completed,
    Unavailable,
    Failed,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchNativeEvent {
    pub name: String,
    pub content: Value,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub status: WebSearchStatus,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub model: Option<ModelId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<WebSearchEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_events: Vec<WebSearchNativeEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub checked_at: DateTime<Utc>,
}

impl WebSearchResult {
    pub fn unavailable(query: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: WebSearchStatus::Unavailable,
            query: query.into(),
            provider: None,
            model: None,
            answer: None,
            evidence: Vec::new(),
            native_events: Vec::new(),
            error: Some(message.into()),
            checked_at: Utc::now(),
        }
    }
}
