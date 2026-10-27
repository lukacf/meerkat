//! Deadline-bounded member observation across transient owner-lane contention.
//!
//! This retries only refused reads. Control admission and delivery remain with
//! the original caller, which keeps its pinned member binding and deadline.

use std::future::Future;
use std::time::Duration;

use meerkat_mob::{AgentIdentity, MobError, MobHandle, MobMemberSnapshot};
use tokio::time::{Instant, sleep_until, timeout_at};

/// Read a fresh owner snapshot without treating a temporarily occupied
/// observation lane as a terminal control failure. The deadline also bounds
/// each individual read; dropping this future cancels its pending read/backoff.
pub(crate) async fn observe_member_status_until(
    handle: &MobHandle,
    identity: &AgentIdentity,
    deadline: Instant,
) -> Result<MobMemberSnapshot, MobError> {
    retry_member_status_observation(deadline, || handle.member_status(identity)).await
}

/// Bound an owner read that may include member-status observation. `read` must
/// not mutate or submit work, and any inner wait must share this same deadline.
pub(crate) async fn retry_member_status_observation<T, Read, ReadFuture>(
    deadline: Instant,
    mut read: Read,
) -> Result<T, MobError>
where
    Read: FnMut() -> ReadFuture,
    ReadFuture: Future<Output = Result<T, MobError>>,
{
    let timeout = || MobError::ActorCommandTimedOut {
        command_kind: "MemberStatus",
        stage: "member_status_observation",
    };
    let mut backoff = Duration::from_millis(10);
    loop {
        if Instant::now() >= deadline {
            return Err(timeout());
        }
        match timeout_at(deadline, read()).await {
            Ok(Err(MobError::LifecycleOperationAdmissionPending { intent, stage }))
                if intent == "member_status_observation"
                    && stage == "observation_lane_saturated" =>
            {
                sleep_until((Instant::now() + backoff).min(deadline)).await;
                backoff = (backoff * 2).min(Duration::from_millis(100));
            }
            Ok(result) => return result,
            Err(_) => return Err(timeout()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::future::{pending, ready};
    use std::rc::Rc;

    use super::*;

    fn observation_contention() -> MobError {
        MobError::LifecycleOperationAdmissionPending {
            intent: "member_status_observation".to_string(),
            stage: "observation_lane_saturated",
        }
    }

    fn assert_observation_timeout<T: std::fmt::Debug>(result: Result<T, MobError>) {
        assert!(matches!(
            result,
            Err(MobError::ActorCommandTimedOut {
                command_kind: "MemberStatus",
                stage: "member_status_observation",
            })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn immediate_owner_snapshot_is_returned_without_a_retry_or_delay() {
        let start = Instant::now();
        let calls = Cell::new(0);
        let result = retry_member_status_observation(start + Duration::from_secs(30), || {
            calls.set(calls.get() + 1);
            ready(Ok(
                "fresh owner snapshot with exact external-member classification",
            ))
        })
        .await;

        assert_eq!(
            result.expect("initial observation succeeds"),
            "fresh owner snapshot with exact external-member classification"
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(Instant::now(), start);
    }

    #[tokio::test(start_paused = true)]
    async fn exact_observer_contention_retries_reads_until_the_owner_answers() {
        let start = Instant::now();
        let calls = Cell::new(0);
        let mut replies = VecDeque::from([
            Err(observation_contention()),
            Err(observation_contention()),
            Ok("fresh external member snapshot"),
        ]);
        let result = retry_member_status_observation(start + Duration::from_secs(30), || {
            calls.set(calls.get() + 1);
            ready(
                replies
                    .pop_front()
                    .expect("no read after successful snapshot"),
            )
        })
        .await;

        assert_eq!(
            result.expect("third read succeeds"),
            "fresh external member snapshot"
        );
        assert_eq!(calls.get(), 3);
        assert!(replies.is_empty());
        assert!(
            Instant::now() > start,
            "contention must yield instead of spin"
        );
        assert!(Instant::now() < start + Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn every_other_owner_error_returns_untouched_without_retry() {
        let errors = [
            MobError::LifecycleOperationAdmissionPending {
                intent: "member_resume".to_string(),
                stage: "observation_lane_saturated",
            },
            MobError::LifecycleOperationAdmissionPending {
                intent: "member_status_observation".to_string(),
                stage: "actor_command_admission",
            },
            MobError::Internal("observation_lane_saturated: member_status_observation".to_string()),
            MobError::MemberNotFound(AgentIdentity::from("original-member")),
            MobError::ActorCommandTimedOut {
                command_kind: "OwnerCommand",
                stage: "original_owner_stage",
            },
        ];

        for error in errors {
            let original_variant = std::mem::discriminant(&error);
            let original_message = error.to_string();
            let start = Instant::now();
            let calls = Cell::new(0);
            let mut reply = Some(error);
            let result = retry_member_status_observation::<(), _, _>(
                start + Duration::from_secs(30),
                || {
                    calls.set(calls.get() + 1);
                    ready(Err(reply.take().expect("unrelated error must not retry")))
                },
            )
            .await;

            let actual = result.expect_err("owner error must not become a snapshot");
            assert_eq!(std::mem::discriminant(&actual), original_variant);
            assert_eq!(actual.to_string(), original_message);
            assert_eq!(calls.get(), 1);
            assert_eq!(Instant::now(), start);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn owner_rejection_after_contention_is_not_retried_or_replaced() {
        let calls = Cell::new(0);
        let mut replies = VecDeque::from([
            Err(observation_contention()),
            Err(MobError::MemberNotFound(AgentIdentity::from(
                "original-member",
            ))),
        ]);
        let result = retry_member_status_observation::<(), _, _>(
            Instant::now() + Duration::from_secs(30),
            || {
                calls.set(calls.get() + 1);
                ready(replies.pop_front().expect("no read after owner rejection"))
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(MobError::MemberNotFound(identity))
                if identity.as_str() == "original-member"
        ));
        assert_eq!(calls.get(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn exhausted_budget_does_not_start_a_read() {
        for deadline in [Instant::now(), Instant::now() - Duration::from_secs(1)] {
            let calls = Cell::new(0);
            let result = retry_member_status_observation(deadline, || {
                calls.set(calls.get() + 1);
                ready(Ok(()))
            })
            .await;

            assert_observation_timeout(result);
            assert_eq!(calls.get(), 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_contention_uses_bounded_backoff_and_the_original_deadline() {
        let start = Instant::now();
        let deadline = start + Duration::from_millis(351);
        let mut attempts = Vec::new();
        let result = retry_member_status_observation::<(), _, _>(deadline, || {
            attempts.push(Instant::now());
            ready(Err(observation_contention()))
        })
        .await;

        assert_observation_timeout(result);
        assert_eq!(Instant::now(), deadline);
        assert!(
            attempts.len() >= 3,
            "contention should receive bounded read retries"
        );
        assert!(
            attempts.len() < 20,
            "contention must not cause a hot read loop"
        );
        assert!(attempts.iter().all(|attempt| *attempt < deadline));
        for pair in attempts.windows(2) {
            let delay = pair[1] - pair[0];
            assert!(delay >= Duration::from_millis(10));
            assert!(delay <= Duration::from_millis(100));
        }
    }

    struct DropProbe(Rc<Cell<usize>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_read_is_dropped_at_the_original_deadline_even_after_contention() {
        for contend_first in [false, true] {
            let deadline = Instant::now() + Duration::from_millis(25);
            let calls = Cell::new(0);
            let drops = Rc::new(Cell::new(0));
            let result = retry_member_status_observation::<(), _, _>(deadline, || {
                calls.set(calls.get() + 1);
                let refuse = contend_first && calls.get() == 1;
                let probe = DropProbe(Rc::clone(&drops));
                async move {
                    let _probe = probe;
                    if refuse {
                        return Err(observation_contention());
                    }
                    pending::<Result<(), MobError>>().await
                }
            })
            .await;

            assert_observation_timeout(result);
            assert_eq!(Instant::now(), deadline);
            assert_eq!(calls.get(), if contend_first { 2 } else { 1 });
            assert_eq!(
                drops.get(),
                calls.get(),
                "no hanging read survives its caller"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_during_backoff_stops_all_further_read_attempts() {
        let calls = Cell::new(0);
        let mut observation = Box::pin(retry_member_status_observation::<(), _, _>(
            Instant::now() + Duration::from_secs(30),
            || {
                calls.set(calls.get() + 1);
                ready(Err(observation_contention()))
            },
        ));
        assert!(futures::poll!(&mut observation).is_pending());
        assert_eq!(calls.get(), 1);

        drop(observation);
        tokio::time::advance(Duration::from_mins(1)).await;
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_drops_an_inflight_read_without_spawning_a_retry() {
        let calls = Cell::new(0);
        let drops = Rc::new(Cell::new(0));
        let mut observation = Box::pin(retry_member_status_observation::<(), _, _>(
            Instant::now() + Duration::from_secs(30),
            || {
                calls.set(calls.get() + 1);
                let probe = DropProbe(Rc::clone(&drops));
                async move {
                    let _probe = probe;
                    pending::<Result<(), MobError>>().await
                }
            },
        ));
        assert!(futures::poll!(&mut observation).is_pending());
        assert_eq!(calls.get(), 1);
        assert_eq!(drops.get(), 0);

        drop(observation);
        assert_eq!(drops.get(), 1);
        tokio::time::advance(Duration::from_mins(1)).await;
        assert_eq!(calls.get(), 1);
    }
}
