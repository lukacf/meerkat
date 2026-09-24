//! Protocol surface enumeration.

use serde::{Deserialize, Serialize};

/// Protocol surfaces supported by Meerkat.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum::EnumIter,
    strum::EnumString,
    strum::Display,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Protocol {
    Rpc,
    Rest,
    Mcp,
    Cli,
}
