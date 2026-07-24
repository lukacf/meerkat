//! Safe wire projection for durable detached jobs.

pub fn project_job_receipt(receipt: meerkat_jobs::JobReceipt) -> meerkat_contracts::JobReceipt {
    meerkat_contracts::JobReceipt {
        job_id: receipt.job_id.to_string(),
        status: project_phase(receipt.phase),
        deduplicated: receipt.deduplicated,
        restart_class: project_restart_class(receipt.restart_class),
        awaitable: true,
    }
}

pub fn project_job_description(
    description: meerkat_jobs::JobDescription,
) -> meerkat_contracts::JobSummary {
    meerkat_contracts::JobSummary {
        job_id: description.job_id.to_string(),
        runner: meerkat_contracts::JobRunner {
            name: description.runner.name().to_string(),
            version: description.runner.version().to_string(),
        },
        phase: project_phase(description.phase),
        restart_class: project_restart_class(description.restart_class),
        attempt_count: description.attempt_count,
        progress: description
            .progress
            .map(|progress| meerkat_contracts::JobProgress {
                cursor: progress.cursor,
                detail: progress.detail,
            }),
        cancel_requested: description.cancel_requested,
        terminal_result: description.terminal_result.map(project_terminal_result),
        subscription_count: description.subscription_count,
        delivery_backlog: description.delivery_backlog,
    }
}

fn project_phase(phase: meerkat_jobs::JobPhase) -> meerkat_contracts::JobPhase {
    use meerkat_contracts::JobPhase as Wire;
    use meerkat_jobs::JobPhase as Domain;
    match phase {
        Domain::Unsubmitted => Wire::Unsubmitted,
        Domain::Queued => Wire::Queued,
        Domain::Claimed => Wire::Claimed,
        Domain::Running => Wire::Running,
        Domain::WaitingExternal => Wire::WaitingExternal,
        Domain::LossObserved => Wire::LossObserved,
        Domain::RetryScheduled => Wire::RetryScheduled,
        Domain::Succeeded => Wire::Succeeded,
        Domain::Failed => Wire::Failed,
        Domain::Cancelled => Wire::Cancelled,
        Domain::WorkerLost => Wire::WorkerLost,
        Domain::NeedsAttention => Wire::NeedsAttention,
    }
}

fn project_restart_class(
    restart_class: meerkat_jobs::RestartClass,
) -> meerkat_contracts::JobRestartClass {
    use meerkat_contracts::JobRestartClass as Wire;
    use meerkat_jobs::RestartClass as Domain;
    match restart_class {
        Domain::Adoptable => Wire::Adoptable,
        Domain::CheckpointResumable => Wire::CheckpointResumable,
        Domain::Replayable => Wire::Replayable,
        Domain::NonResumable => Wire::NonResumable,
    }
}

fn project_terminal_result(
    result: meerkat_jobs::JobTerminalResult,
) -> meerkat_contracts::JobTerminalResult {
    match result {
        meerkat_jobs::JobTerminalResult::Succeeded { result_ref } => {
            meerkat_contracts::JobTerminalResult::Succeeded {
                result_ref: result_ref.map(|value| value.to_string()),
            }
        }
        meerkat_jobs::JobTerminalResult::Failed { code, detail_ref } => {
            meerkat_contracts::JobTerminalResult::Failed {
                code: code.to_string(),
                detail_ref: detail_ref.map(|value| value.to_string()),
            }
        }
        meerkat_jobs::JobTerminalResult::Cancelled => {
            meerkat_contracts::JobTerminalResult::Cancelled
        }
        meerkat_jobs::JobTerminalResult::WorkerLost => {
            meerkat_contracts::JobTerminalResult::WorkerLost
        }
        meerkat_jobs::JobTerminalResult::NeedsAttention { reason } => {
            meerkat_contracts::JobTerminalResult::NeedsAttention {
                reason: reason.to_string(),
            }
        }
    }
}
