//! Bounded observation of the existing original source's acceptance. This is
//! outside native publication/identity calls and never owns a mutation guard.
use super::*;
use tokio::time::{Duration, Instant};

impl ToolkitOriginalOwner {
    /// Wait for this same source to settle Candidate, lock contention or an
    /// unresolved cancellation. Dropped custody and committed cancellation
    /// refuse immediately; observation never changes either disposition.
    /// Every pass runs the same complete strict check as check_admitted_helper;
    /// Candidate is never usable contributor authority. No guard crosses await.
    pub(crate) async fn wait_admitted_helper(
        self: &Arc<Self>, admitted: &AdmittedHelperWork, deadline: Instant,
    ) -> Result<(), OriginalWriterError> {
        if !Arc::ptr_eq(self, &admitted.owner) {
            return Err(OriginalWriterError::WrongOwner);
        }
        loop {
            let observed = self.observe_helper_sources(
                &admitted.contributor, &admitted.contributors, &[],
            )?;
            if Instant::now() >= deadline {
                return Err(OriginalWriterError::Unavailable);
            }
            if observed == HelperSourceSettlement::Admitted {
                return Ok(());
            }
            // Every transient state retains the original absolute deadline.
            // A short timer yields without depending on the tool turn or an
            // acceptance callback channel; the same source owner is reread.
            tokio::time::sleep_until(std::cmp::min(
                deadline, Instant::now() + Duration::from_millis(1),
            )).await;
        }
    }
}

#[cfg(test)]
mod tests;
