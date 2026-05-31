// Hand-maintained aggregator for files written by xtask codegen passes.
// Each sibling module is emitted by either `xtask machine-codegen` or
// `xtask protocol-codegen` — see each file's own header for provenance.
// This aggregator is not itself a codegen output: it is a stable
// `pub mod` index, and `xtask audit-generated-headers` forbids the
// codegen marker here to keep that honest.

pub mod approval_lifecycle;
pub mod auth_lease_durable_lifecycle_marker;
pub mod auth_lease_transition_authority_sources;
pub mod comms_trust_authority_sources;
pub mod pending_continuation_admission;
pub mod protocol_ops_barrier_satisfaction;
pub mod protocol_tool_visibility_owner;
pub mod session_document;
pub mod session_durable_config_authority;
pub mod session_persistence_version_authority;
pub mod session_realtime_transcript_authority;
pub mod terminal_surface_mapping;
