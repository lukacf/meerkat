//! Panic-payload recovery and once-per-distinct-payload log gating for
//! `catch_unwind` boundaries.
//!
//! Shared by the meerkat-runtime machine attachment boundaries
//! (`meerkat-runtime/src/panic_boundary.rs`) and the meerkat-mob actor task
//! boundaries (`meerkat-mob/src/runtime/panic_capture.rs`). std-only on
//! purpose: this must stay reachable from every crate that catches a panic.
//!
//! WHY (field incident, 2026-07-29): a panic inside a member-provisioning
//! transaction was caught at an actor-task `catch_unwind` boundary with the
//! payload discarded, converted to an opaque "task panicked" error, and
//! immediately retried — 40+ minutes at 99% CPU across three releases, with
//! each iteration paying for DWARF symbolication (gimli, ~280MB binary)
//! inside the panic hook, whose stderr line the deployment piped to
//! /dev/null. A swallowed payload plus an eager retry is a furnace; every
//! catch site must recover the payload, log it once per distinct payload
//! (never per iteration), and feed a typed error.
//!
//! panic=abort compatibility: under `panic = "abort"` `catch_unwind` never
//! observes a panic, so these helpers only run when unwinding actually
//! delivered a payload; nothing here assumes a panic happened otherwise.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// Best-effort human-readable panic payload: `&str` and `String` payloads
/// (everything `panic!`/`assert!` produce) are recovered verbatim; anything
/// else (a `panic_any` value) degrades to a fixed marker rather than
/// disappearing.
pub fn panic_payload_detail(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

/// Once-per-distinct-payload log gate, keyed by a caller-chosen context key
/// (member identity, `boundary:session`, ...).
///
/// Retry loops can legally re-run a panicking operation indefinitely; the
/// payload must be logged, and a per-iteration `error!` would be its own
/// flood. This records the last payload per key and reports a first sighting
/// only on transition (new key, or a changed payload). A success at the same
/// boundary should [`clear`](Self::clear) the key so the next distinct
/// incident logs fresh.
#[derive(Debug, Default)]
pub struct PanicPayloadLogGate {
    last_payload_by_key: Mutex<BTreeMap<String, String>>,
}

impl PanicPayloadLogGate {
    /// Record the payload for this key; `true` when it differs from the
    /// previously recorded one (i.e. this transition should be logged).
    pub fn first_sighting(&self, key: &str, detail: &str) -> bool {
        let mut guard = self
            .last_payload_by_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.get(key) {
            Some(previous) if previous == detail => false,
            _ => {
                guard.insert(key.to_string(), detail.to_string());
                true
            }
        }
    }

    /// Forget the key's recorded payload (called on success) so a later
    /// recurrence of the same panic logs again as a new incident.
    pub fn clear(&self, key: &str) {
        self.last_payload_by_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_detail_recovers_str_string_and_degrades_other_types() {
        assert_eq!(panic_payload_detail(&"boom"), "boom");
        assert_eq!(panic_payload_detail(&"boom".to_string()), "boom");
        assert_eq!(panic_payload_detail(&42_u32), "non-string panic payload");
    }

    #[test]
    fn gate_reports_transitions_only_and_clears() {
        let gate = PanicPayloadLogGate::default();
        assert!(gate.first_sighting("k", "a"));
        assert!(!gate.first_sighting("k", "a"));
        assert!(gate.first_sighting("k", "b"));
        assert!(gate.first_sighting("other", "b"));
        gate.clear("k");
        assert!(gate.first_sighting("k", "b"));
    }
}
