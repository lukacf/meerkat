//! Compatibility-only bridge machine schemas.
//!
//! These are intentionally excluded from the canonical two-kernel catalog.
//!
//! Clippy allows below are scoped to this compat/ module because the bridge
//! machine builders are generated-style code: they use `expect()` on local
//! invariants that cannot fail, and carry wire-shape imports the submodules
//! selectively use. These allows stay scoped to the bridge modules so their
//! retirement remains a mechanical deletion target.

// Scoped clippy allows: compat bridge machines are generated-style builders.
// They use `expect()` on invariants that cannot fail in this construction path,
// and carry a standard wire-shape import prelude that submodules selectively
// use. Keep this scoped here so compat surface retirement is a mechanical deletion.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]
mod auth_lease_bridge;
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]
mod external_tool_surface_bridge;
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]
mod mob_destroy_session_ingress_bridge;
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]
mod ops_barrier_bridge;
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]
mod supervisor_trust_bridge;

pub use auth_lease_bridge::auth_lease_bridge_machine;
pub use external_tool_surface_bridge::external_tool_surface_bridge_machine;
pub use mob_destroy_session_ingress_bridge::mob_destroy_session_ingress_bridge_machine;
pub use ops_barrier_bridge::ops_barrier_bridge_machine;
pub use supervisor_trust_bridge::supervisor_trust_bridge_machine;
