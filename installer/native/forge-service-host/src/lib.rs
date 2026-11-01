//! Forge's compiled native collection adapters. No caller/receipt is constructed
//! here. Protected application entry is source-only; qualification is pending.
#![forbid(unsafe_code)]
mod catalog;
mod history;
mod program;
#[cfg(target_os = "linux")]
mod protected_catalog;

pub use catalog::{GheRepository, ProgramLimits, ProgramSpec};
pub use history::{HistoricalCollection, HistoricalObservation, HistoricalStore, ParsedHistory};
pub use program::{AdapterError, BoundGheProgram, GheProgram};
#[cfg(target_os = "linux")]
pub use protected_catalog::{
    APPLICATION_ID, CATALOG_ID, ForgeGheCatalog, SOURCE_AGENT, SOURCE_FAMILY, SOURCE_PROFILE,
    SOURCE_SELECTION,
};
