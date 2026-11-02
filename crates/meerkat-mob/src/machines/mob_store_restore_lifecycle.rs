//! Production owner module for the canonical `MobStoreRestoreLifecycleMachine`.
//!
//! The lifecycle body is catalog-owned
//! (`meerkat-machine-schema::catalog::dsl::mob_store_restore_lifecycle`); this
//! module is the production binding the machine-authority registry names.
//! Keep runtime and store mechanics out of this file: the machine's semantics
//! have exactly one home.
//!
//! Ownership: a quarantined persisted mob store is mob domain authority (the
//! restore of a realm's persisted mobs). Mob owns it. The mob-set state in
//! `meerkat-mob-mcp` runs the restores the machine admits and holds one
//! record per quarantined store in memory, but does not own the domain.
//!
//! The machine is record-scoped and volatile: one machine state per
//! quarantined store, never persisted.

pub use meerkat_machine_schema::catalog::dsl::mob_store_restore_lifecycle::*;
