#![allow(clippy::expect_used)]

use meerkat_jobs::machines::detached_job::{
    DetachedJobInput, DetachedJobMachineAuthority, DetachedJobMachineMutator, DetachedJobPhase,
};

fn submitted() -> DetachedJobMachineAuthority {
    let mut authority = DetachedJobMachineAuthority::new();
    DetachedJobMachineMutator::apply(
        &mut authority,
        DetachedJobInput::Submit {
            job_id: "job-1".into(),
            restart_class: meerkat_jobs::machines::detached_job::DetachedJobRestartClass::Adoptable,
        },
    )
    .expect("submit");
    authority
}

fn claim(authority: &mut DetachedJobMachineAuthority, attempt: &str, now: u64) {
    DetachedJobMachineMutator::apply(
        authority,
        DetachedJobInput::ClaimAttempt {
            attempt_id: attempt.into(),
            worker_id: format!("worker-{attempt}"),
            claimed_at_ms: now,
            lease_expires_at_ms: now + 100,
            runner_handle: format!("runner-{attempt}"),
        },
    )
    .expect("claim");
}

#[test]
fn recovery_is_identity_preserving_and_only_claim_mints_attempt_and_fence() {
    let mut authority = submitted();
    claim(&mut authority, "attempt-1", 1);
    let before_reopen = authority.state().clone();

    let reopened =
        DetachedJobMachineAuthority::recover_from_state(before_reopen.clone()).expect("recover");
    assert_eq!(reopened.state(), &before_reopen);

    let mut reopened = reopened;
    DetachedJobMachineMutator::apply(
        &mut reopened,
        DetachedJobInput::LeaseExpired {
            attempt_id: "attempt-1".into(),
            fence: 1,
            observed_at_ms: 102,
        },
    )
    .expect("lease expiry");
    assert_eq!(reopened.state().attempt_count, 1);
    assert_eq!(reopened.state().current_fence, 1);

    DetachedJobMachineMutator::apply(
        &mut reopened,
        DetachedJobInput::ScheduleRetry {
            retry_due_at_ms: 110,
        },
    )
    .expect("schedule retry");
    assert_eq!(reopened.state().attempt_count, 1);
    assert_eq!(reopened.state().current_fence, 1);

    claim(&mut reopened, "attempt-2", 110);
    assert_eq!(reopened.state().attempt_count, 2);
    assert_eq!(reopened.state().current_fence, 2);
}

#[test]
fn waiting_external_writes_preserve_the_waiting_phase() {
    let mut authority = submitted();
    claim(&mut authority, "attempt-1", 1);
    DetachedJobMachineMutator::apply(
        &mut authority,
        DetachedJobInput::WaitExternal {
            attempt_id: "attempt-1".into(),
            fence: 1,
            observed_at_ms: 55,
        },
    )
    .expect("wait");

    for input in [
        DetachedJobInput::RenewLease {
            attempt_id: "attempt-1".into(),
            fence: 1,
            heartbeat_at_ms: 50,
            lease_expires_at_ms: 150,
        },
        DetachedJobInput::ReportProgress {
            attempt_id: "attempt-1".into(),
            fence: 1,
            cursor: 1,
            observed_at_ms: 60,
        },
        DetachedJobInput::RecordCheckpoint {
            attempt_id: "attempt-1".into(),
            fence: 1,
            checkpoint_ref: "checkpoint:1".into(),
            observed_at_ms: 60,
        },
    ] {
        DetachedJobMachineMutator::apply(&mut authority, input).expect("waiting write");
        assert_eq!(
            authority.state().lifecycle_phase,
            DetachedJobPhase::WaitingExternal
        );
    }
}

#[test]
fn checkpoint_and_wait_transitions_reject_observations_after_the_committed_lease() {
    let mut authority = submitted();
    claim(&mut authority, "attempt-1", 1);

    DetachedJobMachineMutator::apply(
        &mut authority,
        DetachedJobInput::RecordCheckpoint {
            attempt_id: "attempt-1".into(),
            fence: 1,
            checkpoint_ref: "checkpoint:late".into(),
            observed_at_ms: 102,
        },
    )
    .expect_err("checkpoint after the committed lease must be rejected");
    DetachedJobMachineMutator::apply(
        &mut authority,
        DetachedJobInput::WaitExternal {
            attempt_id: "attempt-1".into(),
            fence: 1,
            observed_at_ms: 102,
        },
    )
    .expect_err("external wait after the committed lease must be rejected");
    assert_eq!(authority.state().lifecycle_phase, DetachedJobPhase::Running);
}

#[test]
fn lease_renewal_is_monotonic_and_replayed_older_renewals_are_rejected() {
    let mut authority = submitted();
    claim(&mut authority, "attempt-1", 1);
    DetachedJobMachineMutator::apply(
        &mut authority,
        DetachedJobInput::RenewLease {
            attempt_id: "attempt-1".into(),
            fence: 1,
            heartbeat_at_ms: 50,
            lease_expires_at_ms: 200,
        },
    )
    .expect("first renewal");

    for input in [
        DetachedJobInput::RenewLease {
            attempt_id: "attempt-1".into(),
            fence: 1,
            heartbeat_at_ms: 40,
            lease_expires_at_ms: 250,
        },
        DetachedJobInput::RenewLease {
            attempt_id: "attempt-1".into(),
            fence: 1,
            heartbeat_at_ms: 60,
            lease_expires_at_ms: 150,
        },
    ] {
        DetachedJobMachineMutator::apply(&mut authority, input)
            .expect_err("heartbeat and lease expiry must never regress");
    }
    assert_eq!(authority.state().heartbeat_at_ms, Some(50));
    assert_eq!(authority.state().lease_expires_at_ms, Some(200));
}

#[test]
fn checkpoint_resumable_loss_cannot_schedule_a_retry_without_a_committed_checkpoint() {
    let mut authority = DetachedJobMachineAuthority::new();
    DetachedJobMachineMutator::apply(
        &mut authority,
        DetachedJobInput::Submit {
            job_id: "job-checkpoint".into(),
            restart_class:
                meerkat_jobs::machines::detached_job::DetachedJobRestartClass::CheckpointResumable,
        },
    )
    .expect("submit");
    claim(&mut authority, "attempt-1", 1);
    DetachedJobMachineMutator::apply(
        &mut authority,
        DetachedJobInput::LeaseExpired {
            attempt_id: "attempt-1".into(),
            fence: 1,
            observed_at_ms: 102,
        },
    )
    .expect("lease expiry");

    DetachedJobMachineMutator::apply(
        &mut authority,
        DetachedJobInput::ScheduleRetry {
            retry_due_at_ms: 110,
        },
    )
    .expect_err("checkpoint-resumable retry requires committed resume identity");
    assert_eq!(
        authority.state().lifecycle_phase,
        DetachedJobPhase::LossObserved
    );
    assert_eq!(authority.state().current_fence, 1);
}
