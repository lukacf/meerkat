//! Capability wire projections for Meerkat.
//!
//! The typed capability vocabulary and registry mechanics live in
//! `meerkat-capabilities`. Feature crates submit their own declarations there;
//! this module re-exports the shared types for public contract consumers and
//! owns only the wire response shapes in [`query`].

pub mod query;

pub use meerkat_capabilities::{
    BrowserMobpackCapabilityDecision, CapabilityId, CapabilityProtocol, CapabilityRegistration,
    CapabilityScope, CapabilityStatus, DeploySurfaceCapabilityId, FeatureCapabilityPolicy,
    HostProcessCapabilityId, MobpackCapabilityId, MobpackCapabilityRequirement,
    available_capabilities, browser_mobpack_capability_decision, build_capabilities,
    resolve_capabilities,
};
pub use query::{CapabilitiesResponse, CapabilityEntry};
