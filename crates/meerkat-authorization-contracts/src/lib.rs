//! Portable data contracts and restriction algebra for host-trusted authorization.
//!
//! These claims and restriction calculations confer no authentication,
//! permission or currentness. Canonical feature owners retain grant, policy
//! and operation authority. No runtime producer or persistence is provided here.

pub mod audit;
pub mod constraints;
pub mod derived_child;
pub mod evidence;
pub mod grant;
pub mod grant_mutation;
pub mod protocol;
pub mod resource;
pub mod work_association;

#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod constraints_tests;
