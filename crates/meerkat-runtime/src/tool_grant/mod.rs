//! Scoped tool grants.
//!
//! One grant opens tool names a session build marked grant-gated, for a
//! bounded window. The canonical `ToolGrantLifecycleMachine` owns each
//! grant's lifecycle; see `dsl.rs` for its production instantiation and
//! `crates/meerkat-machine-schema/src/catalog/dsl/tool_grant_lifecycle.rs`
//! for the catalog body (the two must stay structurally identical; the
//! schema parity test enforces this).

#[allow(
    dead_code,
    clippy::assign_op_pattern,
    clippy::cmp_owned,
    clippy::unused_self,
    clippy::unnecessary_map_on_constructor
)]
pub mod dsl;
