//! Forge's compiled native collection adapters. No caller/receipt is constructed
//! here. The protected application entry and installed qualification are pending.
mod catalog;
mod history;
mod program;

pub use catalog::{GheRepository, ProgramLimits, ProgramSpec};
pub use history::{HistoricalCollection, HistoricalObservation, HistoricalStore, ParsedHistory};
pub use program::{AdapterError, BoundGheProgram, GheProgram};
