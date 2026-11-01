//! Wire result wrappers for `workgraph/*` read surfaces.
//!
//! The item/event element schemas are owned by `meerkat-workgraph`; these
//! wrappers fix the response envelope shape (K17) so surfaces serialize a
//! named contract instead of hand-shaping `{"items": ...}` JSON.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Result envelope for work-item list reads (`workgraph/list`, `workgraph/ready`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkItemsResult {
    pub items: Vec<Value>,
    /// Items that matched the filter at this read (for `workgraph/ready`, the
    /// ready items), before `limit` cut them. Absent only from a server that
    /// predates the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_items: Option<usize>,
    /// Matching items exist after the last item returned. Absent only from a
    /// server that predates the field, which does not mean the page is
    /// complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// Result envelope for work-event list reads (`workgraph/events`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkEventsResult {
    pub events: Vec<Value>,
}
