// Hand-maintained aggregator for files written by xtask codegen passes.
// Each sibling module is emitted by either `xtask machine-codegen` or
// `xtask protocol-codegen` — see each file's own header for provenance.
// This aggregator is not itself a codegen output: it is a stable
// `pub mod` index, and `xtask audit-generated-headers` forbids the
// codegen marker here to keep that honest.

pub mod flow_frame;
pub mod flow_run;
pub mod loop_iteration;
pub mod protocol_mob_destroying_session_ingress;
