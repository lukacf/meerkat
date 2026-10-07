//! Exact resource coordinates for operation targets and historical audit data.
//!
//! A coordinate is neither permission nor a semantic dependency claim.

use serde::{Deserialize, Serialize};

use crate::constraints::ResourceDomain;

/// Exact resource identity within an authority-owned domain.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRef {
    pub domain: ResourceDomain,
    pub resource_id: String,
}
