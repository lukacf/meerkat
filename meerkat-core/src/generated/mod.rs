// Hand-maintained aggregator for files written by xtask codegen passes.
// Each sibling module is emitted by either `xtask machine-codegen` or
// `xtask protocol-codegen` — see each file's own header for provenance.
// This aggregator is not itself a codegen output: it is a stable
// `pub mod` index, and `xtask audit-generated-headers` forbids the
// codegen marker here to keep that honest.

pub mod protocol_ops_barrier_satisfaction;
pub mod terminal_surface_mapping;
