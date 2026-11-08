//! A test witness seam, not an API. A host's test build may install one
//! plain function, which every durable write method of
//! [`SqliteStore`](crate::SqliteStore) calls with the method's name in the
//! caller's task, before the write starts. Nothing is installed in
//! production, and then the seam does nothing.

use std::sync::OnceLock;

static WITNESS: OnceLock<fn(&'static str)> = OnceLock::new();

/// Install `witness` for this process; the first installation stays. A test
/// witness seam only.
pub fn install(witness: fn(&'static str)) {
    let _ = WITNESS.set(witness);
}

/// Call the installed witness, if any, with the write's name.
pub(crate) fn witness(write: &'static str) {
    if let Some(witness) = WITNESS.get() {
        witness(write);
    }
}
