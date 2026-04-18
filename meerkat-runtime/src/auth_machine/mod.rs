//! AuthMachine runtime surface.
//!
//! Per-binding auth-lease lifecycle machine. Each `binding_key`
//! (format `"<realm_id>:<binding_id>"`) has its own `AuthMachine`
//! instance, managed by the per-binding registry in
//! `meerkat-runtime/src/handles/auth_lease.rs`.
//!
//! See `dsl.rs` for the machine definition. See
//! `meerkat-machine-schema/src/catalog/dsl/auth_machine.rs` for the
//! schema-catalog mirror (the two must stay structurally identical;
//! the xtask drift-check enforces this).

#[allow(
    dead_code,
    clippy::assign_op_pattern,
    clippy::cmp_owned,
    clippy::unused_self,
    clippy::unnecessary_map_on_constructor
)]
pub mod dsl;
