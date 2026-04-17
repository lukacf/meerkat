//! Typed decision helper for the bind-fallback path.
//!
//! When a mob sends a bridge command that requires an already-bound
//! supervisor (e.g. `AuthorizeSupervisor`) and the member replies with a
//! `Rejected` that indicates the supervisor state does not match the
//! mob's own authority (`NotBound`, `StaleSupervisor`, `SenderMismatch`),
//! the correct recovery is to fall back to a fresh `BindMember` call —
//! restoring membership by re-running the bootstrap path.
//!
//! Before this helper landed, the same `matches!` pattern was duplicated at
//! three callsites (`actor.rs` twice, `provisioner.rs` once) and had
//! previously been implemented via string-matching on `reason.contains(...)`
//! of the rejection message, which was fragile because any edit to those
//! strings silently broke rotation fallback (see the H1 review finding).
//! Consolidating here ensures every callsite uses the same typed predicate.
//!
//! Extending the fallback set is intentionally a single-point change: add
//! the variant here and all callsites pick it up.

use super::bridge_protocol::BridgeRejectionCause;

/// Returns `true` when a bridge rejection indicates the member's
/// supervisor state is out of sync with the mob and the mob should
/// recover by re-running `BindMember`.
///
/// The three causes in the fallback set share the property that the
/// member is reachable and speaking the protocol correctly, but its
/// supervisor authority is either missing or mismatched relative to the
/// mob's current authority. Every other cause (`AlreadyBound`,
/// `UnsupportedProtocolVersion`, `InvalidBootstrapToken`,
/// `InvalidSupervisorSpec`, `InvalidPeerSpec`, `AddressMismatch`,
/// `Unsupported`, `Internal`) is a hard failure that a fresh
/// `BindMember` would not fix, and so must bubble up.
pub(super) fn should_fall_back_to_bind(cause: BridgeRejectionCause) -> bool {
    matches!(
        cause,
        BridgeRejectionCause::NotBound
            | BridgeRejectionCause::StaleSupervisor
            | BridgeRejectionCause::SenderMismatch,
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_on_not_bound_stale_and_sender_only() {
        assert!(should_fall_back_to_bind(BridgeRejectionCause::NotBound));
        assert!(should_fall_back_to_bind(
            BridgeRejectionCause::StaleSupervisor
        ));
        assert!(should_fall_back_to_bind(
            BridgeRejectionCause::SenderMismatch
        ));
    }

    #[test]
    fn does_not_fall_back_on_hard_failures() {
        let hard_failures = [
            BridgeRejectionCause::AlreadyBound,
            BridgeRejectionCause::InvalidBootstrapToken,
            BridgeRejectionCause::UnsupportedProtocolVersion,
            BridgeRejectionCause::InvalidSupervisorSpec,
            BridgeRejectionCause::InvalidPeerSpec,
            BridgeRejectionCause::AddressMismatch,
            BridgeRejectionCause::Unsupported,
            BridgeRejectionCause::Internal,
        ];
        for cause in hard_failures {
            assert!(
                !should_fall_back_to_bind(cause),
                "cause {cause:?} must not trigger bind fallback"
            );
        }
    }
}
