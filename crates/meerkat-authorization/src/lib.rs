//! Local operation authorization composed with the process-local grant owner.
//!
//! The generated grant owner is the sole mutable grant authority. Native work
//! and policy owners supply independent current facts; prepared decisions remain
//! disposable projections bound to the same local publication and clock.
//! These checks do not track semantic information flow or provide persistence,
//! consent or operating-system confinement.

mod audit;
pub mod clock;
pub mod grant_policy;
pub mod grants;
pub mod policy;
pub mod publication;
pub mod work;
