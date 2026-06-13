// Hand-maintained aggregator for files written by xtask codegen passes.
// Each sibling module is emitted by either `xtask machine-codegen` or
// `xtask protocol-codegen` — see each file's own header for provenance.
// This aggregator is not itself a codegen output: it is a stable
// `pub mod` index, and `xtask audit-generated-headers` forbids the
// codegen marker here to keep that honest.

pub mod adaptive_mob_bundle;
pub mod catalog_input;
pub mod protocol_mob_destroying_session_ingress;
pub mod protocol_mob_external_peer_reciprocal_trust;
pub mod protocol_mob_external_peer_trust_repair;
pub mod protocol_mob_external_peer_trust_unwiring;
pub mod protocol_mob_external_peer_trust_wiring;
pub mod protocol_mob_member_peer_overlay;
pub mod protocol_mob_member_trust_unwiring;
pub mod protocol_mob_member_trust_wiring;
