//! Local operation authorization composed with the process-local grant owner.
//!
//! The generated grant owner is the sole mutable grant authority. Native work
//! and policy owners supply independent current facts; prepared decisions remain
//! disposable projections bound to the same local publication and clock.
//! These checks do not track semantic information flow or provide consent or
//! operating-system confinement. The only persistence is the grant owner's
//! optional restart custody; prepared decisions are never persisted.

// Without `workspace-tests` (the published package, whose test command
// leaves it off) the unit tests that need the workspace compile out, and
// the test helpers only they use go unused. The workspace's test builds
// turn the feature on and keep both lints.
#![cfg_attr(
    all(test, not(feature = "workspace-tests")),
    allow(dead_code, unused_imports)
)]

mod audit;
pub mod clock;
pub mod grant_policy;
pub mod grants;
pub mod mcp_installation;
pub mod policy;
pub mod publication;
pub mod work;
