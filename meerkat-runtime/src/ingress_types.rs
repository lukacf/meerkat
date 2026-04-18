//! Wire-type shells preserved from the deleted runtime ingress authority.
//!
//! These types name admission metadata that persists beyond the authority
//! itself. They are pure data — no authority methods, no shadow state. The
//! DSL owns ingress semantics (queue lanes, input phases, admission
//! ordering); these types just carry content-shape / correlation metadata
//! from the admission point to observability readers.

/// Content shape classification for admitted inputs.
///
/// Used by the admitted-input snapshot surface so callers can correlate
/// admissions by content type without re-parsing the Input payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentShape(pub String);

/// Reservation key for admitted inputs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReservationKey(pub String);

/// Request ID for correlation tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);
