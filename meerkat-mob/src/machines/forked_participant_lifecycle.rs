//! Production owner module for the canonical `ForkedParticipantLifecycleMachine`.
//!
//! The lifecycle body is catalog-owned
//! (`meerkat-machine-schema::catalog::dsl::forked_participant_lifecycle`); this
//! module is the production binding the machine-authority registry names. Keep
//! runtime/store mechanics out of this file — the machine's semantics have
//! exactly one home.
//!
//! Ownership: a forked-participant capability is multi-agent domain authority
//! (a detached, scoped, expiring participation grant that a temporary mob
//! attaches). Mob owns it. A runtime host executes the authority's verdicts
//! mechanically but does not own the domain.
//!
//! The machine is record-scoped: one machine state per source-owned forked
//! participant capability record.

pub use meerkat_machine_schema::catalog::dsl::forked_participant_lifecycle::*;
