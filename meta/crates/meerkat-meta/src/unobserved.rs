//! The one shape for a fact a record holds no current observation of.
//!
//! A fact Meta has not observed is never given a value: the record carries
//! it as null, or withholds it, and names it in `unobserved` with the
//! reason, `"unobserved": {"reason": ..., "fields": [...]}`. The console
//! experience's records and its live snapshot share it.

use serde::{Deserialize, Serialize};

/// Why the named facts of a record hold no current observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnobservedReason {
    /// The identity binding behind the record moved, and the console's read
    /// model had not re-collected it by the binding refresh deadline.
    RefreshPending,
    /// Nothing has observed the fact yet, so there is no answer to report.
    NotObserved,
}

/// The facts of a record that hold no current observation, and why. Build
/// it with [`UnobservedFacts::new`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UnobservedFacts {
    pub reason: UnobservedReason,
    /// The record's field names, as they appear on the wire.
    pub fields: Vec<String>,
}

impl UnobservedFacts {
    pub fn new(reason: UnobservedReason, fields: &[&str]) -> Self {
        Self {
            reason,
            fields: fields.iter().map(|field| (*field).to_string()).collect(),
        }
    }

    /// The JSON form a record embeds under `unobserved`.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::json!({ "reason": self.reason, "fields": self.fields })
    }
}
