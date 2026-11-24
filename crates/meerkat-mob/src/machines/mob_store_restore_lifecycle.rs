//! Production owner module for the canonical `MobStoreRestoreLifecycleMachine`.
//!
//! The lifecycle body is catalog-owned
//! (`meerkat-machine-schema::catalog::dsl::mob_store_restore_lifecycle`); this
//! module is the production binding the machine-authority registry names.
//! Keep runtime and store mechanics out of this file: the machine's semantics
//! have exactly one home.
//!
//! Ownership: one mob store's stay in a host's mob set (dormant, loading,
//! managed, unloading, held, quarantined, retrying, gone) is mob domain
//! authority. Mob owns it. The mob-set state in `meerkat-mob-mcp` holds one
//! record per store in memory, runs the loads, re-attempts and unloads the
//! machine admits, and reads its sets from the records, but does not own the
//! domain.
//!
//! The machine is record-scoped and volatile: one machine state per store,
//! never persisted.

pub use meerkat_machine_schema::catalog::dsl::mob_store_restore_lifecycle::*;
