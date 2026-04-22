// @generated — protocol helper modules
//
// Each module wraps a handoff protocol declared in a composition schema.
// Protocol helpers enforce obligation tracking: every effect that requires
// owner feedback returns an obligation token that must be consumed by the
// corresponding feedback submitter.

pub mod flow_frame;
pub mod flow_run;
pub mod loop_iteration;
pub mod protocol_flow_loop_until_evaluation;
