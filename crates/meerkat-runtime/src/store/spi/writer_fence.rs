//! Production owner module for the canonical `HostStoreWriterFenceMachine`.
//!
//! The lifecycle body is catalog-owned
//! (`meerkat-machine-schema::catalog::dsl::host_store_writer_fence`); this
//! module is the production binding the machine-authority registry names.
//! The realm ([`super::SpiRealm`]) holds one machine state per open realm and
//! drives it with the answers of its store calls and the facts of its clock.
//! Keep store and clock mechanics out of this file: the machine's semantics
//! have exactly one home.

pub use meerkat_machine_schema::catalog::dsl::host_store_writer_fence::*;
