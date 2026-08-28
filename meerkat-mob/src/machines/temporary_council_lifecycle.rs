//! Production owner module for the canonical `TemporaryCouncilLifecycleMachine`.
//!
//! The lifecycle body is catalog-owned
//! (`meerkat-machine-schema::catalog::dsl::temporary_council_lifecycle`); this
//! module is the production binding the machine-authority registry names. Keep
//! runtime/store mechanics out of this file — the machine's semantics have
//! exactly one home.
//!
//! Ownership: a temporary council is multi-agent domain authority (a bounded
//! conversation between forked-participant capabilities seated in a real,
//! short-lived mob). Mob owns it. The mob-MCP coordinator executes the
//! authority's verdicts mechanically but does not own the domain.
//!
//! The machine is record-scoped: one machine state per temporary-council
//! record.

pub use meerkat_machine_schema::catalog::dsl::temporary_council_lifecycle::*;
