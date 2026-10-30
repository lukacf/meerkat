//! Durable decision runtime surface.
//!
//! One `DurableDecisionMachine` instance per durable decision record. The
//! machine owns the decision's lifecycle: the hold and its sealed binding,
//! the presentation of the held subject, the attested decision, the
//! execution permit and the executed fact, expiry, withdrawal and the owed
//! requester notice.
//!
//! See `dsl.rs` for the machine instantiation and
//! `crates/meerkat-machine-schema/src/catalog/dsl/durable_decision.rs` for
//! the canonical declaration (the two must stay structurally identical; the
//! xtask drift check enforces this).

#[allow(
    dead_code,
    unused_variables,
    unreachable_code,
    clippy::assign_op_pattern,
    clippy::cmp_owned,
    clippy::unused_self,
    clippy::unnecessary_map_on_constructor
)]
pub mod dsl;
