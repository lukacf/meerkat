//! Test builds: the write witness over this crate's store writes, and a log
//! of the store writes made inside protected sections.
//!
//! The store calls the witness at each durable write method, in the writer's
//! task. A write inside a governed bind attempt that has neither passed its
//! decision point nor recorded its effect mark fails there, naming the write.

use meerkat_core::protected_section::{self, ProtectedSection};
use std::sync::{Mutex, PoisonError};

/// One store write made inside a protected section, as the witness saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WitnessedWrite {
    pub(crate) write: &'static str,
    pub(crate) section: ProtectedSection,
    pub(crate) effect: Option<&'static str>,
    pub(crate) decided: Option<&'static str>,
}

static LOG: Mutex<Vec<WitnessedWrite>> = Mutex::new(Vec::new());

/// Install the witness for this test process; the first installation stays.
pub(crate) fn install() {
    toolkit_store::write_witness::install(witness);
}

fn witness(write: &'static str) {
    if let Some(section) = protected_section::current() {
        LOG.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(WitnessedWrite {
                write,
                section,
                effect: protected_section::effect_recorded(),
                decided: protected_section::decision_recorded(),
            });
    }
    protected_section::assert_effect_marked(write);
}

/// Take the writes logged so far.
pub(crate) fn take() -> Vec<WitnessedWrite> {
    std::mem::take(&mut *LOG.lock().unwrap_or_else(PoisonError::into_inner))
}
