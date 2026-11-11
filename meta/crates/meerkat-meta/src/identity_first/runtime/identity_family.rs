//! Where the identity runtime keeps the family of changes and reads of its
//! mob's long-term agents (`identity_creation::IdentityFamily`).
//!
//! A launch that has an identity policy attaches the family once, after the
//! runtime is built. Every surface reads it here at each call: the stdio
//! gateway's and the Console's `meta/identity/*` methods, and the agents'
//! identity tools, which are mounted exactly while a family is attached. A
//! launch without a policy attaches none: the methods are not served and no
//! tool is mounted. The family is detached when the mob stops, with the
//! identity authority.

use std::sync::Arc;

use super::IdentityRuntime;
use crate::identity_creation::IdentityFamily;

impl IdentityRuntime {
    /// Attach `family`: every surface of the mob calls it from now on.
    pub fn attach_identity_family(&self, family: Arc<dyn IdentityFamily>) {
        self.identity_family.attach(family);
    }

    /// The attached family: `None` on a launch without an identity policy,
    /// and once the mob stopped.
    pub fn identity_family(&self) -> Option<Arc<dyn IdentityFamily>> {
        self.identity_family.get()
    }

    /// Detach the family: the mob stopped.
    pub(crate) fn detach_identity_family(&self) {
        self.identity_family.clear();
    }
}
