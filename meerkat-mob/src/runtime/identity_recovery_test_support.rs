//! Deterministic fail-stop hooks for the identity recovery compatibility gate.
//!
//! This module is compiled only for crate tests or the explicit
//! `test-support` feature. Production reconciliation contains no crash-hook
//! state and never consults this module for authority.

use std::sync::{Arc, Mutex, OnceLock};

/// A durable-write frontier exercised by the cross-process recovery matrix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityRecoveryFailStopPoint {
    ManifestCommittedBeforeReply,
    ManifestReplySent,
    LeaseAcquired,
    RuntimeCasApplied,
    MemberEventApplied,
}

type IdentityRecoveryFailStopHook = Arc<dyn Fn() + Send + Sync + 'static>;

static IDENTITY_RECOVERY_FAIL_STOP_HOOK: OnceLock<
    Mutex<Option<(IdentityRecoveryFailStopPoint, IdentityRecoveryFailStopHook)>>,
> = OnceLock::new();

/// Arms one process-local, one-shot fail-stop hook.
///
/// The hook is removed before it is invoked, so even a panic cannot leave a
/// reusable crash authority behind. Subprocess tests normally use a closure
/// that calls `std::process::exit`.
#[doc(hidden)]
pub fn arm_identity_recovery_fail_stop_for_test(
    point: IdentityRecoveryFailStopPoint,
    hook: IdentityRecoveryFailStopHook,
) -> Result<(), &'static str> {
    let mut armed = IDENTITY_RECOVERY_FAIL_STOP_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "identity recovery fail-stop hook mutex is poisoned")?;
    if armed.is_some() {
        return Err("identity recovery fail-stop hook is already armed");
    }
    *armed = Some((point, hook));
    Ok(())
}

pub(crate) fn trigger_identity_recovery_fail_stop(point: IdentityRecoveryFailStopPoint) {
    let hook = {
        let Ok(mut armed) = IDENTITY_RECOVERY_FAIL_STOP_HOOK
            .get_or_init(|| Mutex::new(None))
            .lock()
        else {
            return;
        };
        match armed.as_ref() {
            Some((armed_point, _)) if *armed_point == point => armed.take().map(|(_, hook)| hook),
            _ => None,
        }
    };
    if let Some(hook) = hook {
        hook();
    }
}
