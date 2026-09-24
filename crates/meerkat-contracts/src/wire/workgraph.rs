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
}

/// Result envelope for work-event list reads (`workgraph/events`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkEventsResult {
    pub events: Vec<Value>,
}
