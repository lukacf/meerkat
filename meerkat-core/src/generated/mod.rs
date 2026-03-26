// @generated — protocol helper modules
//
// Each module wraps a handoff protocol declared in a composition schema.
// Protocol helpers enforce obligation tracking: every effect that requires
// owner feedback returns an obligation token that must be consumed by the
// corresponding feedback submitter.

pub mod protocol_comms_drain_abort;
pub mod protocol_comms_drain_spawn;
pub mod protocol_ops_barrier_satisfaction;
pub mod terminal_surface_mapping;
