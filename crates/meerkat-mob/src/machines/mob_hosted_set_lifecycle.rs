//! Production owner module for the canonical `MobHostedSetLifecycleMachine`.
//!
//! The lifecycle body is catalog-owned
//! (`meerkat-machine-schema::catalog::dsl::mob_hosted_set_lifecycle`); this
//! module is the production binding the machine-authority registry names.
//! Keep runtime and store mechanics out of this file: the machine's semantics
//! have exactly one home.
//!
//! Ownership: the facts about a host's mob set as a whole (how far the pass
//! over the stores the host has from before came, whether a census of them
//! is owed and complete, whether a host inserts its own handles, whether
//! every mob is known) are mob domain authority. Mob owns them. The mob-set
//! state in `meerkat-mob-mcp` holds the one state of its set in memory, runs
//! the pass the machine admits, and reads these facts from the machine, but
//! does not own the domain.
//!
//! The machine is set-scoped and volatile: one machine state per mob set,
//! never persisted.

pub use meerkat_machine_schema::catalog::dsl::mob_hosted_set_lifecycle::*;
