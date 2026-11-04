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

// Test-feature canary. This crate's tests build with its test features,
// which no dev-dependency turns on: in this repository every test command
// passes them (`node scripts/core-workspace-args.mjs --tests -p meerkat-authorization`
// prints them). A test build without them would compile the tests they
// gate out and pass over fewer tests, so it fails to compile here instead.
// `.cargo/config.toml` [env] sets MEERKAT_WORKSPACE_TEST_FEATURES for this
// repository's builds only; the published package builds without it.
#[cfg(all(test, not(feature = "workspace-tests")))]
const _: () = assert!(
    option_env!("MEERKAT_WORKSPACE_TEST_FEATURES").is_none(),
    "meerkat-authorization: this repository's test builds need `--features workspace-tests` \
     (core-workspace-args.mjs --tests -p meerkat-authorization prints them)"
);

mod audit;
pub mod clock;
pub mod grant_policy;
pub mod grants;
pub mod mcp_installation;
pub mod policy;
pub mod publication;
pub mod work;
