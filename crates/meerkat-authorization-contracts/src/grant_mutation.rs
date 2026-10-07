//! Trusted native custody around a grant administration operation.
//!
//! This contract creates no permission or native state. Its production
//! implementation must retain the actual native owners across the callback.

use crate::grant::GrantLineageRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerCustodyRefusal {
    /// An unfinished work item retains this reference in its controller lineage.
    ControllerInUse,
    Unavailable,
}

/// Complete attached native scope, held across the actual grant mutation.
///
/// Implementations are trusted owner integrations. They must inspect every
/// actual attached owner's accepted rows and generated lifecycle while holding
/// the same custody that excludes admission, recovery and owner replacement.
/// Veto only when `reference` occurs in the complete retained controller lineage
/// of unfinished work, including its ancestors. Ordinary operation delegation,
/// authority-basis or work-use references do not create this veto. They remain
/// revocable even while work is unfinished. This check grants no permission to
/// the caller; the grant owner still validates the actor and exact mutation.
///
/// A cached count, detached boolean, audit record or partial scope is not an
/// implementation. Missing, busy or unavailable ownership yields `Unavailable`,
/// not `ControllerInUse` or an operation permission verdict.
///
/// The callback runs synchronously inside that custody and at most once. It
/// acquires the grant publication and owner locks after native custody, never
/// before awaiting a native lock. No permissive production implementation is
/// supplied by the contracts or grant feature crates.
pub trait ControllerGrantMutationCustody {
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        reference: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ControllerCustodyRefusal>;
}
