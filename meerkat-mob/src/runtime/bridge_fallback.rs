//! Typed decision helper for the bind-fallback path.
//!
//! When a mob sends a bridge command that requires an already-bound
//! supervisor (e.g. `AuthorizeSupervisor`) and the member replies with a
//! rejection whose [`BridgeRejectionClass`] is
//! [`BridgeRejectionClass::RecoverableBySupervisorRebind`], the correct
//! recovery is to fall back to a fresh `BindMember` call — restoring
//! membership by re-running the bootstrap path.
//!
//! Classification authority lives on [`BridgeRejectionCause::class`] in
//! `meerkat-contracts`, not here. This helper is a thin adapter that
//! projects the typed class onto a boolean for the callsites that want
//! exactly the "should I retry via bind?" question. Any future class
//! variant (e.g. a "retry after backoff" case) is added to the typed
//! class in contracts, and every callsite picks it up through the
//! protocol types — the hardcoded cause set that used to live here has
//! been deleted.

use super::bridge_protocol::{BridgeRejectionCause, BridgeRejectionClass};

/// Returns `true` when a bridge rejection indicates the member's
/// supervisor state is out of sync with the mob and the mob should
/// recover by re-running `BindMember`. Branches on the typed
/// [`BridgeRejectionClass`] from the contracts crate — no cause-set
/// literal comparison lives in this helper.
pub(super) fn should_fall_back_to_bind(cause: BridgeRejectionCause) -> bool {
    matches!(
        cause.class(),
        BridgeRejectionClass::RecoverableBySupervisorRebind
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_on_recoverable_class() {
        assert!(should_fall_back_to_bind(BridgeRejectionCause::NotBound));
        assert!(should_fall_back_to_bind(
            BridgeRejectionCause::StaleSupervisor
        ));
        assert!(should_fall_back_to_bind(
            BridgeRejectionCause::SenderMismatch
        ));
    }

    #[test]
    fn does_not_fall_back_on_fatal_class() {
        let fatal = [
            BridgeRejectionCause::AlreadyBound,
            BridgeRejectionCause::InvalidBootstrapToken,
            BridgeRejectionCause::UnsupportedProtocolVersion,
            BridgeRejectionCause::InvalidSupervisorSpec,
            BridgeRejectionCause::InvalidPeerSpec,
            BridgeRejectionCause::AddressMismatch,
            BridgeRejectionCause::Unsupported,
            BridgeRejectionCause::Internal,
        ];
        for cause in fatal {
            assert!(
                !should_fall_back_to_bind(cause),
                "cause {cause:?} must not trigger bind fallback"
            );
        }
    }

    #[test]
    fn class_partitions_causes_cleanly() {
        // Defensive regression: every `BridgeRejectionCause` variant must
        // have a concrete class. If a new variant is added and forgotten
        // here, the test below will fail to enumerate it.
        let recoverable = [
            BridgeRejectionCause::NotBound,
            BridgeRejectionCause::StaleSupervisor,
            BridgeRejectionCause::SenderMismatch,
        ];
        for cause in recoverable {
            assert_eq!(
                cause.class(),
                BridgeRejectionClass::RecoverableBySupervisorRebind,
                "cause {cause:?} must be in the recoverable class"
            );
        }
        let fatal = [
            BridgeRejectionCause::AlreadyBound,
            BridgeRejectionCause::InvalidBootstrapToken,
            BridgeRejectionCause::UnsupportedProtocolVersion,
            BridgeRejectionCause::InvalidSupervisorSpec,
            BridgeRejectionCause::InvalidPeerSpec,
            BridgeRejectionCause::AddressMismatch,
            BridgeRejectionCause::Unsupported,
            BridgeRejectionCause::Internal,
        ];
        for cause in fatal {
            assert_eq!(
                cause.class(),
                BridgeRejectionClass::Fatal,
                "cause {cause:?} must be in the fatal class"
            );
        }
    }
}
