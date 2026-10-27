//! Cleanup state in the existing application driver. The attempts factory and
//! returned owner remain the original preparation's custody throughout close.
use super::*;
use meerkat_mcp::McpGuardCompletion;

const JOIN_BOUND: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct LocalRoundCleanup {
    owner: Option<LocalRoundOwner>,
    owner_clean: bool,
    native_drained: bool,
    failed: bool,
}

impl LocalRoundCleanup {
    pub(crate) async fn join(
        &mut self,
        attempts: &LocalRoundAttempts,
        guards: &McpAttemptGuards,
        deadline: tokio::time::Instant,
    ) -> Result<(), GatewayHostError> {
        // close_and_join forgets ended calls. Record their failure before any
        // next await so a later empty report cannot erase that observation.
        let records = guards.close_and_join(deadline).await;
        let before_deadline = tokio::time::Instant::now() < deadline;
        let mut pending = false;
        for record in records {
            match record.completion {
                McpGuardCompletion::Completed | McpGuardCompletion::Cancelled => (),
                McpGuardCompletion::Held | McpGuardCompletion::TimedOut => pending = true,
                McpGuardCompletion::Failed | McpGuardCompletion::Panicked => self.failed = true,
                _ => { self.failed = true; pending = true; },
            }
        }
        self.native_drained = before_deadline && !pending;
        if self.owner.is_none() {
            self.owner = attempts.take_closed_owner();
        }
        if let Some(owner) = self.owner.as_mut() {
            if !self.owner_clean {
                // The owner is stored outside this borrowed future. Timeout or
                // caller cancellation leaves its exact Child for the next join.
                match tokio::time::timeout_at(deadline, owner.cancel_pending_round()).await {
                    Ok(Ok(())) => self.owner_clean = !owner.has_pending_round(),
                    Ok(Err(_)) => self.failed = true,
                    Err(_) => (),
                }
            }
        }
        if self.native_drained && self.owner_clean && !self.failed {
            Ok(())
        } else {
            Err(GatewayHostError::Unavailable)
        }
    }

    pub(crate) async fn join_until_settled(
        &mut self,
        attempts: &LocalRoundAttempts,
        guards: &McpAttemptGuards,
    ) -> Result<(), GatewayHostError> {
        loop {
            let result = self.join(attempts, guards,
                tokio::time::Instant::now() + JOIN_BOUND).await;
            if self.native_drained && self.owner_clean {
                return result;
            }
            // This is still the existing Meta-owned host-close callback. It
            // must not return and drop Driver while actual calls or a Child
            // remain owned. A held process can make whole shutdown unbounded.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[cfg(test)]
    pub(crate) fn take_owner_for_test(&mut self) -> Option<LocalRoundOwner> {
        self.owner.take()
    }
}

impl Driver {
    pub(super) async fn join_local_round(&self) -> Result<(), GatewayHostError> {
        let mut cleanup = self.local_round_cleanup.lock().await;
        cleanup.join_until_settled(&self.local_round_attempts,
            &self.attempt_guards).await
    }
}

// Before the Driver exists, no factory can enter this owner. Still release it
// explicitly on every fallible preparation path instead of treating Drop as join.
pub(super) async fn preparation_failed(error: String, owner: &mut LocalRoundOwner) -> String {
    match owner.cancel_pending_round().await {
        Ok(()) => error,
        Err(cleanup) => format!("{error}; local preparation cleanup also failed: {cleanup}"),
    }
}
