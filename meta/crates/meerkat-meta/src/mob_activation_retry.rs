//! In-process retry of a stalled explicit mob resume.
//!
//! Activating a prepared mob runs Meerkat's explicit Resume. Meerkat bounds
//! each observation of that operation by a compile-time patience window: when
//! one member's host-owned work makes no observable progress for that long,
//! the caller receives [`MobError::LifecycleOperationProgressStalled`]. The
//! operation itself is not abandoned. It stays registered as the current
//! process-owned Resume for the handle's command authority, its driver task
//! keeps running, and a later `MobHandle::resume` on the same handle joins it
//! instead of starting a new one.
//!
//! On a memory-starved host one member can legitimately exceed the window.
//! Treating the stall as fatal shuts the runtime down and restarts the whole
//! boot from scratch, which re-does exactly the work that was slow. This loop
//! re-joins instead, for as long as the operation keeps moving: every stall
//! names the member and stage it stopped at, and a changed member or stage is
//! progress. Only [`ActivationStallPolicy::max_consecutive_stalls`] stalls in a
//! row at the same point, or the overall stall cap, give up.

use std::future::Future;
use std::time::Instant;

use meerkat_mob::{AgentIdentity, MobError};

/// Env var that widens or narrows the consecutive-stall bound.
pub(crate) const ACTIVATION_STALL_RETRIES_ENV: &str = "META_ACTIVATION_STALL_RETRIES";

/// Default consecutive stalls tolerated at one (member, stage). With Meerkat's
/// 30 s patience window this is about five minutes without progress on one
/// member.
pub(crate) const DEFAULT_ACTIVATION_STALL_RETRIES: u32 = 10;

/// Upper clamp for the env override. Past this an operator wants an unbounded
/// boot, which this bound deliberately does not offer.
const MAX_ACTIVATION_STALL_RETRIES: u32 = 1_000;

/// Overall stall cap as a multiple of the consecutive bound. Progress resets
/// the consecutive count, so without an overall cap a pair of points that
/// alternated forever would never give up.
const TOTAL_STALLS_PER_CONSECUTIVE_BOUND: u32 = 20;

/// How many stalls an activation tolerates before it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActivationStallPolicy {
    max_consecutive_stalls: u32,
}

impl ActivationStallPolicy {
    /// The policy from [`ACTIVATION_STALL_RETRIES_ENV`], or the default.
    pub(crate) fn from_env() -> Self {
        Self::parse(std::env::var(ACTIVATION_STALL_RETRIES_ENV).ok().as_deref())
    }

    /// Parse an env value: an integer clamped to `[1, 1000]`; unset or
    /// unparseable falls back to [`DEFAULT_ACTIVATION_STALL_RETRIES`].
    fn parse(raw: Option<&str>) -> Self {
        let max_consecutive_stalls = raw
            .and_then(|value| value.trim().parse::<u32>().ok())
            .map(|value| value.clamp(1, MAX_ACTIVATION_STALL_RETRIES))
            .unwrap_or(DEFAULT_ACTIVATION_STALL_RETRIES);
        Self {
            max_consecutive_stalls,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_max_consecutive_stalls(max_consecutive_stalls: u32) -> Self {
        Self {
            max_consecutive_stalls: max_consecutive_stalls.clamp(1, MAX_ACTIVATION_STALL_RETRIES),
        }
    }

    /// Consecutive stalls at one (member, stage) that fail the activation.
    pub(crate) fn max_consecutive_stalls(self) -> u32 {
        self.max_consecutive_stalls
    }

    /// Stalls across all points that fail the activation.
    pub(crate) fn max_total_stalls(self) -> u32 {
        self.max_consecutive_stalls
            .saturating_mul(TOTAL_STALLS_PER_CONSECUTIVE_BOUND)
    }
}

impl Default for ActivationStallPolicy {
    fn default() -> Self {
        Self {
            max_consecutive_stalls: DEFAULT_ACTIVATION_STALL_RETRIES,
        }
    }
}

/// Where a stalled Resume stopped, as Meerkat reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StallPoint {
    member_id: Option<AgentIdentity>,
    stage: &'static str,
}

/// Run `resume` and re-run it after each progress stall, within `policy`.
///
/// `resume` must join the same Meerkat operation on every call (it is
/// `MobHandle::resume` on the one prepared handle in production). Any error
/// other than [`MobError::LifecycleOperationProgressStalled`] is returned
/// immediately, as is the stall that exhausts the policy.
pub(crate) async fn resume_joining_stalled_operation<F, Fut>(
    policy: ActivationStallPolicy,
    mut resume: F,
) -> Result<(), MobError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), MobError>>,
{
    let started_at = Instant::now();
    let mut last_point: Option<StallPoint> = None;
    let mut consecutive_stalls: u32 = 0;
    let mut total_stalls: u32 = 0;
    loop {
        match resume().await {
            Ok(()) => {
                if total_stalls > 0 {
                    tracing::info!(
                        stalls = total_stalls,
                        elapsed_ms = started_at.elapsed().as_millis(),
                        "mob activation completed after re-joining a stalled explicit resume"
                    );
                }
                return Ok(());
            }
            Err(MobError::LifecycleOperationProgressStalled {
                intent,
                member_id,
                stage,
            }) => {
                let point = StallPoint {
                    member_id: member_id.clone(),
                    stage,
                };
                consecutive_stalls = if last_point.as_ref() == Some(&point) {
                    consecutive_stalls.saturating_add(1)
                } else {
                    1
                };
                total_stalls = total_stalls.saturating_add(1);
                last_point = Some(point);
                let exhausted = consecutive_stalls >= policy.max_consecutive_stalls()
                    || total_stalls >= policy.max_total_stalls();
                tracing::warn!(
                    member = ?member_id,
                    stage,
                    attempt = total_stalls,
                    consecutive_stalls,
                    max_consecutive_stalls = policy.max_consecutive_stalls(),
                    elapsed_ms = started_at.elapsed().as_millis(),
                    giving_up = exhausted,
                    env = ACTIVATION_STALL_RETRIES_ENV,
                    "mob activation explicit resume stalled; re-joining the same operation"
                );
                if exhausted {
                    return Err(MobError::LifecycleOperationProgressStalled {
                        intent,
                        member_id,
                        stage,
                    });
                }
            }
            Err(other) => return Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn stall(member: &str, stage: &'static str) -> MobError {
        MobError::LifecycleOperationProgressStalled {
            intent: "explicit_resume".to_string(),
            member_id: Some(AgentIdentity::from(member)),
            stage,
        }
    }

    /// Replays scripted outcomes, then succeeds; counts calls.
    async fn run_script(
        policy: ActivationStallPolicy,
        script: Vec<MobError>,
    ) -> (Result<(), MobError>, usize) {
        let mut script: VecDeque<MobError> = script.into();
        let mut calls = 0usize;
        let result = resume_joining_stalled_operation(policy, || {
            calls += 1;
            let next = script.pop_front();
            async move { next.map_or(Ok(()), Err) }
        })
        .await;
        (result, calls)
    }

    #[tokio::test]
    async fn a_stall_that_later_resolves_completes() {
        let (result, calls) = run_script(
            ActivationStallPolicy::with_max_consecutive_stalls(3),
            vec![stall("a", "resume_member"), stall("a", "resume_member")],
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(calls, 3);
    }

    #[tokio::test]
    async fn k_consecutive_stalls_at_one_point_fail_typed() {
        let (result, calls) = run_script(
            ActivationStallPolicy::with_max_consecutive_stalls(3),
            vec![
                stall("a", "resume_member"),
                stall("a", "resume_member"),
                stall("a", "resume_member"),
                stall("a", "resume_member"),
            ],
        )
        .await;
        assert_eq!(calls, 3, "gives up on the Kth stall, not later");
        assert!(matches!(
            result,
            Err(MobError::LifecycleOperationProgressStalled {
                member_id: Some(ref member),
                stage: "resume_member",
                ..
            }) if member.as_str() == "a"
        ));
    }

    #[tokio::test]
    async fn a_changed_member_or_stage_is_progress() {
        // K=2: each point stalls once or twice, never twice in a row twice.
        let (result, calls) = run_script(
            ActivationStallPolicy::with_max_consecutive_stalls(2),
            vec![
                stall("a", "resume_member"),
                stall("b", "resume_member"),
                stall("b", "retire_member"),
                stall("a", "retire_member"),
            ],
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(calls, 5);
    }

    #[tokio::test]
    async fn alternating_points_hit_the_overall_cap() {
        let policy = ActivationStallPolicy::with_max_consecutive_stalls(2);
        let cap = policy.max_total_stalls() as usize;
        let script = (0..cap + 5)
            .map(|index| {
                if index % 2 == 0 {
                    stall("a", "resume_member")
                } else {
                    stall("b", "resume_member")
                }
            })
            .collect();
        let (result, calls) = run_script(policy, script).await;
        assert_eq!(calls, cap);
        assert!(matches!(
            result,
            Err(MobError::LifecycleOperationProgressStalled { .. })
        ));
    }

    #[tokio::test]
    async fn a_non_stall_error_is_fatal_immediately() {
        let (result, calls) = run_script(
            ActivationStallPolicy::with_max_consecutive_stalls(10),
            vec![
                MobError::Internal("boom".to_string()),
                stall("a", "resume_member"),
            ],
        )
        .await;
        assert_eq!(calls, 1);
        assert!(matches!(result, Err(MobError::Internal(ref message)) if message == "boom"));
    }

    #[test]
    fn env_value_parses_with_default_and_clamp() {
        let parse = |raw| ActivationStallPolicy::parse(raw).max_consecutive_stalls();
        assert_eq!(parse(None), DEFAULT_ACTIVATION_STALL_RETRIES);
        assert_eq!(parse(Some("")), DEFAULT_ACTIVATION_STALL_RETRIES);
        assert_eq!(parse(Some("lots")), DEFAULT_ACTIVATION_STALL_RETRIES);
        assert_eq!(parse(Some("-3")), DEFAULT_ACTIVATION_STALL_RETRIES);
        assert_eq!(parse(Some(" 25 ")), 25);
        assert_eq!(parse(Some("0")), 1);
        assert_eq!(parse(Some("999999")), MAX_ACTIVATION_STALL_RETRIES);
        assert_eq!(
            ActivationStallPolicy::default().max_consecutive_stalls(),
            DEFAULT_ACTIVATION_STALL_RETRIES
        );
        assert_eq!(
            ActivationStallPolicy::parse(Some("10")).max_total_stalls(),
            200
        );
    }
}
