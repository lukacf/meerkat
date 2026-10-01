//! Portable data contracts and restriction algebra for host-trusted authorization.
//!
//! These values and compatibility results confer no authentication, permission
//! or currentness. Actual feature owners retain grant and policy authority.
//! No operation enforcement, native admission or persistence is provided here.

pub mod constraints;
pub mod derived_child;
pub mod evidence;
pub mod grant;
pub mod grant_mutation;
pub mod protocol;
pub mod resource;

#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod constraints_tests;
