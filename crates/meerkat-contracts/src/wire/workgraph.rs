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
    /// ready items), before the cursor and `limit` cut them. Absent only from
    /// a server that predates the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_items: Option<usize>,
    /// Matching items exist after the last item returned. Absent only from a
    /// server that predates the field, which does not mean the page is
    /// complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Pass back as `cursor` for the next page. Present exactly when
    /// `truncated` is true, except on a `limit: 0` page, which returns only
    /// the totals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// The event ledger's high-water mark when the walk began: read before
    /// the first page's rows, then carried by the cursor. Absent when the
    /// ledger had no event then, and from a server that predates the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_event_seq: Option<i64>,
    /// The ledger has moved past `anchor_event_seq`: an item can have
    /// changed or appeared since the walk began, and a later page can miss
    /// it. `workgraph/events` with `after_seq` set to `anchor_event_seq`
    /// names every such item. Absent only from a server that predates the
    /// field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_since_anchor: Option<bool>,
}

/// Result envelope for work-event list reads (`workgraph/events`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkEventsResult {
    pub events: Vec<Value>,
    /// Matching events exist after the last event returned. Absent only from
    /// a server that predates the field, which does not mean the page is
    /// complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Pass back as `after_seq` for the next page: the `seq` of the last
    /// event returned. Present exactly when `truncated` is true, except on a
    /// `limit: 0` page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after_seq: Option<i64>,
    /// The ledger's highest sequence in the namespace at this read, internal
    /// execution events included, so it can pass the last public event.
    /// Absent when the ledger is empty, and from a server that predates the
    /// field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_high_water_mark: Option<i64>,
}
