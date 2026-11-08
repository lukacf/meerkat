//! The host's trust decision over the hook sources the factory reads.
//!
//! The factory discovers command hooks in the convention files
//! `<context_root>/.rkat/config.toml` (`<project_root>/.rkat/config.toml`
//! when the factory has no context root) and
//! `<user_config_root>/.rkat/config.toml`, and runs them as the host. A host
//! that gates those files cannot do so by checking them before a build: the
//! factory reads them again while building, so content written in between
//! would run with no decision covering it. A [`HookSourceTrust`] installed with
//! [`crate::AgentFactory::with_hook_source_trust`] is asked instead, by the
//! factory, about the exact bytes it read and parsed, in the same read: the
//! decision binds the content consumed. A source it does not trust keeps its
//! hooks' slots and every invocation of them is refused
//! ([`meerkat_core::confinement::ConfinementRefusal::UntrustedConfigurationSource`]),
//! so none of them runs.

use std::fmt;
use std::path::Path;

use sha2::{Digest as _, Sha256};

/// The SHA-256 of a hook source file's whole content.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HookSourceDigest([u8; 32]);

impl HookSourceDigest {
    /// The digest of `content`, as the factory computes it over the bytes it
    /// reads. A host records a file's digest with this function so its
    /// record and the factory's read agree on the algorithm.
    #[must_use]
    pub fn of(content: &[u8]) -> Self {
        Self(Sha256::digest(content).into())
    }

    /// A digest from its 32 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The digest's 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Lower-case hexadecimal, 64 characters.
impl fmt::Display for HookSourceDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for HookSourceDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HookSourceDigest({self})")
    }
}

/// A hook source file as the factory read it: where, and the digest of the
/// bytes it parsed its hooks from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct HookSource<'a> {
    /// The convention file, as the factory joined it from its root
    /// (`<root>/.rkat/config.toml`).
    pub path: &'a Path,
    /// The SHA-256 of the bytes read.
    pub sha256: HookSourceDigest,
}

/// The host's decision over a [`HookSource`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookSourceDecision {
    /// The hooks this content states may run as the host.
    Trusted,
    /// Every hook this content states is refused at each invocation (an
    /// entry a trusted user file states identically stays the user's).
    Untrusted,
}

/// The host's trust over the hook sources the factory reads.
///
/// Called once per build for each convention file the factory read and
/// parsed, with the digest of exactly those bytes, whether or not the file
/// states a hook and whether or not any of its hooks survives the layering
/// (the build's own config can shadow them all). A file the factory finds
/// absent, cannot read or cannot parse states no hooks and is not asked
/// about. Every build asks again, so a session resumed or woken later, a
/// fork, a helper or a member is judged on the file as that build reads it.
/// Without a trust installed, every source is trusted (unless the host
/// confinement says a confined process could change it).
///
/// The decision covers those bytes only. It does not cover:
///
/// - the program or script a command hook names, which is read when the
///   hook runs: a trusted `sh .rkat/hooks/check.sh` runs whatever the script
///   holds then. A host that needs that bounded confines command hooks
///   ([`crate::AgentFactory::with_command_hook_confinement`]);
/// - a run's hook overrides ([`crate::AgentBuildConfig::hooks_override`]):
///   they are the caller's own input, which the trust does not judge, even
///   when one repeats a convention entry's definition. A run may disable an
///   untrusted entry and supply its own under the same id;
/// - the hooks of the config the host passes to the build, which
///   [`meerkat_core::Config::load`] fills from the project
///   `.rkat/config.toml` (or else the user's). A host that gates the project
///   file by its content does not load its config that way;
/// - the hooks of an engine the host hands the build
///   ([`crate::AgentBuildConfig::hook_engine_override`]): that build reads no
///   convention file and asks nothing. [`crate::resolve_layered_hooks_config`]
///   layers those files with no decision and no host confinement.
///
/// `decide` runs on the build's own task, inside the async build, so it
/// must not block, wait for a person or do I/O: decide from a record kept in
/// memory, such as the digests the host recorded when the content was
/// approved.
pub trait HookSourceTrust: Send + Sync {
    /// Whether the hooks of `source` may run.
    fn decide(&self, source: HookSource<'_>) -> HookSourceDecision;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::HookSourceDigest;

    #[test]
    fn the_digest_is_sha256_in_lower_case_hex() {
        assert_eq!(
            HookSourceDigest::of(b"abc").to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
