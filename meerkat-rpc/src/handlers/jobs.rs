//! Durable detached-job RPC handlers.

use std::path::Path;
use std::sync::Arc;

use meerkat::{
    AttemptId, AttemptWriteAuthority, CheckpointRef, DetachedJobError, FenceToken, JobDeliveryKind,
    JobFailureCode, JobId, JobProgress, JobResultRef, JobSubscription, JobSubscriptionId,
};
use meerkat_contracts::{
    JobArtifactRef, JobAttemptAuthority, JobHealthStatus, JobHealthSummary, JobRestartClass,
    JobsArtifactsParams, JobsArtifactsResult, JobsCancelParams, JobsCancelResult, JobsGetParams,
    JobsGetResult, JobsHealthResult, JobsListParams, JobsListResult, JobsProgressParams,
    JobsProgressResult, JobsResultParams, JobsResultResult, JobsRetryParams, JobsRetryResult,
    JobsSubscribeParams, JobsSubscribeResult, JobsUnsubscribeParams, JobsUnsubscribeResult,
    MobkitJobCancelAckParams, MobkitJobCheckpointParams, MobkitJobCompleteParams,
    MobkitJobFailParams, MobkitJobHeartbeatParams, MobkitJobMutationResult,
    MobkitJobProgressParams, MonitorOutputProtocol, MonitorsStartParams, MonitorsStartResult,
};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

#[allow(clippy::result_large_err)]
fn parse<T: DeserializeOwned>(
    id: Option<RpcId>,
    params: Option<&RawValue>,
) -> Result<T, RpcResponse> {
    parse_params(params).map_err(|response| response.with_id(id))
}

fn job_id(raw: &str) -> Result<JobId, DetachedJobError> {
    JobId::new(raw)
}

fn write_authority(
    authority: JobAttemptAuthority,
) -> Result<(JobId, AttemptWriteAuthority), DetachedJobError> {
    Ok((
        job_id(&authority.job_id)?,
        AttemptWriteAuthority {
            attempt_id: AttemptId::new(authority.attempt_id)?,
            fence: FenceToken::new(authority.fence),
        },
    ))
}

async fn projected(
    runtime: &SessionRuntime,
    job_id: &JobId,
) -> Result<meerkat_contracts::JobSummary, DetachedJobError> {
    described(runtime, job_id)
        .await
        .map(meerkat::project_job_description)
}

async fn described(
    runtime: &SessionRuntime,
    job_id: &JobId,
) -> Result<meerkat::JobDescription, DetachedJobError> {
    let realm_id = runtime.realm_id().ok_or_else(|| {
        DetachedJobError::InvalidInput("durable jobs require an active realm".to_string())
    })?;
    runtime
        .detached_job_service()
        .describe_for_realm(realm_id.as_str(), job_id)
        .await?
        .ok_or_else(|| DetachedJobError::NotFound(job_id.clone()))
}

fn response_error(id: Option<RpcId>, failure: DetachedJobError) -> RpcResponse {
    let code = match failure {
        DetachedJobError::InvalidInput(_)
        | DetachedJobError::NotFound(_)
        | DetachedJobError::InvalidTransition { .. }
        | DetachedJobError::StaleAttempt { .. }
        | DetachedJobError::SubmissionConflict => error::INVALID_PARAMS,
        DetachedJobError::StaleRevision { .. }
        | DetachedJobError::Store(_)
        | DetachedJobError::Sqlite(_) => error::INTERNAL_ERROR,
    };
    RpcResponse::error(id, code, failure.to_string())
}

pub async fn handle_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsGetParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let job_id = match job_id(&params.job_id) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    match projected(runtime, &job_id).await {
        Ok(job) => RpcResponse::success(id, JobsGetResult { job }),
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsListParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let Some(raw_session_id) = params.session_id else {
        return RpcResponse::error(id, error::INVALID_PARAMS, "jobs/list requires session_id");
    };
    let session_id = match meerkat_core::SessionId::parse(&raw_session_id) {
        Ok(session_id) => session_id,
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("invalid jobs/list session_id: {error}"),
            );
        }
    };
    let Some(realm_id) = runtime.realm_id() else {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "jobs/list requires an active realm",
        );
    };
    let limit = usize::try_from(params.limit.unwrap_or(100).min(1_000)).unwrap_or(1_000);
    match runtime
        .detached_job_service()
        .list_descriptions_for_origin(realm_id.as_str(), &session_id, limit)
        .await
    {
        Ok(jobs) => RpcResponse::success(
            id,
            JobsListResult {
                jobs: jobs
                    .into_iter()
                    .map(meerkat::project_job_description)
                    .collect(),
            },
        ),
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_cancel(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsCancelParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let job_id = match job_id(&params.job_id) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    if let Err(error) = described(runtime, &job_id).await {
        return response_error(id, error);
    }
    match runtime.cancel_local_durable_runner(&job_id).await {
        Ok(true) => {
            return match projected(runtime, &job_id).await {
                Ok(job) => RpcResponse::success(id, JobsCancelResult { job }),
                Err(error) => response_error(id, error),
            };
        }
        Ok(false) => {}
        Err(error) => return RpcResponse::error(id, error.code, error.message),
    }
    let service = runtime.detached_job_service();
    match service.request_cancel(&job_id).await {
        Ok(_) => match projected(runtime, &job_id).await {
            Ok(job) => {
                if let Some(dispatcher) = runtime.callback_tool_dispatcher(Vec::new()) {
                    let cancel_job_id = job_id.clone();
                    tokio::spawn(async move {
                        if let Err(error) = dispatcher.cancel_detached_job(&cancel_job_id).await {
                            tracing::warn!(%error, job_id = %cancel_job_id, "detached callback cancellation delivery failed");
                        }
                    });
                }
                RpcResponse::success(id, JobsCancelResult { job })
            }
            Err(error) => response_error(id, error),
        },
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_monitor_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: MonitorsStartParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let session_id =
        match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime.as_ref()) {
            Ok(session_id) => session_id,
            Err(response) => return response,
        };
    if params.timeout_secs == 0 {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            "monitors/start timeout_secs must be greater than zero",
        );
    }
    if params.submission_key.trim().is_empty() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            "monitors/start submission_key must not be empty",
        );
    }
    let protocol = match params.protocol {
        MonitorOutputProtocol::FramedJsonl => {
            meerkat_tools::builtin::shell::MonitorOutputProtocol::FramedJsonl
        }
        MonitorOutputProtocol::Lines => meerkat_tools::builtin::shell::MonitorOutputProtocol::Lines,
    };
    let restart_class = match params.restart_class {
        JobRestartClass::Adoptable => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "agent-authored script monitors cannot claim adoptable restart semantics",
            );
        }
        JobRestartClass::CheckpointResumable => meerkat::RestartClass::CheckpointResumable,
        JobRestartClass::Replayable => meerkat::RestartClass::Replayable,
        JobRestartClass::NonResumable => meerkat::RestartClass::NonResumable,
    };
    let delivery = match params.delivery {
        meerkat_contracts::JobDeliveryKind::Record => JobDeliveryKind::Record,
        meerkat_contracts::JobDeliveryKind::Notification => JobDeliveryKind::Notification,
        meerkat_contracts::JobDeliveryKind::Event { handling_mode } => JobDeliveryKind::Event {
            handling_mode: handling_mode.into(),
        },
    };
    let defaults = meerkat_tools::builtin::shell::MonitorProtocolLimits::default();
    let checked_usize =
        |name: &'static str, value: Option<u64>, default: usize| -> Result<usize, String> {
            match value {
                Some(value) => usize::try_from(value).map_err(|_| {
                    format!("monitors/start {name} exceeds this host's supported size")
                }),
                None => Ok(default),
            }
        };
    let limits = match (
        checked_usize(
            "max_line_bytes",
            params.max_line_bytes,
            defaults.max_line_bytes,
        ),
        checked_usize(
            "max_notifications_per_window",
            params.max_notifications_per_window,
            defaults.max_notifications_per_window,
        ),
        checked_usize(
            "max_retained_diagnostic_bytes",
            params.max_retained_diagnostic_bytes,
            defaults.max_retained_diagnostic_bytes,
        ),
    ) {
        (
            Ok(max_line_bytes),
            Ok(max_notifications_per_window),
            Ok(max_retained_diagnostic_bytes),
        ) => meerkat_tools::builtin::shell::MonitorProtocolLimits {
            max_line_bytes,
            max_notifications_per_window,
            notification_window_ms: params
                .notification_window_ms
                .unwrap_or(defaults.notification_window_ms),
            max_retained_diagnostic_bytes,
        },
        (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => {
            return RpcResponse::error(id, error::INVALID_PARAMS, message);
        }
    };
    let manager = match runtime.monitor_job_manager_for_session(&session_id).await {
        Ok(manager) => manager,
        Err(error) => return RpcResponse::error(id, error.code, error.message),
    };
    let working_dir = params.working_dir.as_deref().map(Path::new);
    let options = meerkat_tools::builtin::shell::MonitorStartOptions {
        protocol,
        restart_class,
        limits,
        delivery,
    };
    let submitted = manager
        .spawn_monitor_for_call(
            &params.command,
            working_dir,
            params.timeout_secs,
            &params.submission_key,
            options,
        )
        .await;
    let submitted = match submitted {
        Ok(job_id) => job_id,
        Err(error) => {
            return RpcResponse::error(id, error::INVALID_PARAMS, error.to_string());
        }
    };
    let job_id = match job_id(submitted.as_ref()) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    match projected(runtime, &job_id).await {
        Ok(job) => RpcResponse::success(id, MonitorsStartResult { job }),
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_get_progress(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsProgressParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let job_id = match job_id(&params.job_id) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    match projected(runtime, &job_id).await {
        Ok(job) => RpcResponse::success(
            id,
            JobsProgressResult {
                job_id: job.job_id,
                phase: job.phase,
                progress: job.progress,
            },
        ),
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_result(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsResultParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let job_id = match job_id(&params.job_id) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    match projected(runtime, &job_id).await {
        Ok(job) => RpcResponse::success(
            id,
            JobsResultResult {
                job_id: job.job_id,
                phase: job.phase,
                result: job.terminal_result,
            },
        ),
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_artifacts(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsArtifactsParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let job_id = match job_id(&params.job_id) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    match described(runtime, &job_id).await {
        Ok(job) => {
            let artifacts = match job.terminal_result {
                Some(meerkat::JobTerminalResult::Succeeded {
                    result_ref: Some(reference),
                }) => vec![JobArtifactRef {
                    reference: reference.to_string(),
                }],
                Some(meerkat::JobTerminalResult::Failed {
                    detail_ref: Some(reference),
                    ..
                }) => vec![JobArtifactRef {
                    reference: reference.to_string(),
                }],
                _ => Vec::new(),
            };
            RpcResponse::success(
                id,
                JobsArtifactsResult {
                    job_id: job.job_id.to_string(),
                    artifacts,
                },
            )
        }
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_retry(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsRetryParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let job_id = match job_id(&params.job_id) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    let service = runtime.detached_job_service();
    if let Err(error) = described(runtime, &job_id).await {
        return response_error(id, error);
    }
    match service
        .schedule_retry(&job_id, params.retry_due_at_ms)
        .await
    {
        Ok(_) => match projected(runtime, &job_id).await {
            Ok(job) => {
                if let Some(dispatcher) = runtime.callback_tool_dispatcher(Vec::new()) {
                    let retry_due_at_ms = params.retry_due_at_ms;
                    tokio::spawn(async move {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
                            .unwrap_or(retry_due_at_ms);
                        if retry_due_at_ms > now_ms {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                retry_due_at_ms - now_ms,
                            ))
                            .await;
                        }
                        if let Err(error) = dispatcher.start_due_detached_jobs().await {
                            tracing::warn!(%error, "scheduled detached callback retry did not start");
                        }
                    });
                }
                RpcResponse::success(id, JobsRetryResult { job })
            }
            Err(error) => response_error(id, error),
        },
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_health(id: Option<RpcId>, runtime: &Arc<SessionRuntime>) -> RpcResponse {
    let Some(realm_id) = runtime.realm_id() else {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "jobs/health requires an active realm",
        );
    };
    let observed_at_ms = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        Err(error) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("system clock is before the Unix epoch: {error}"),
            );
        }
    };
    match runtime
        .detached_job_service()
        .health_snapshot_for_realm(realm_id.as_str(), observed_at_ms, 10_000)
        .await
    {
        Ok(health) => {
            let runtime_backlog = match runtime.runtime_job_delivery_backlog().await {
                Ok(backlog) => backlog,
                Err(error) => {
                    return RpcResponse::error(id, error::INTERNAL_ERROR, error);
                }
            };
            let awaiting_members = match runtime.detached_job_awaiting_member_count().await {
                Ok(awaiting_members) => awaiting_members,
                Err(error) => {
                    return RpcResponse::error(id, error.code, error.message);
                }
            };
            let delivery_backlog = health.delivery_backlog.saturating_add(runtime_backlog);
            RpcResponse::success(
                id,
                JobsHealthResult {
                    detached_jobs: JobHealthSummary {
                        status: if health.is_degraded() || runtime_backlog > 0 {
                            JobHealthStatus::Degraded
                        } else {
                            JobHealthStatus::Ok
                        },
                        queued: health.queued,
                        running: health.running,
                        awaiting_members,
                        stale_leases: health.stale_leases,
                        needs_attention: health.needs_attention,
                        delivery_backlog,
                    },
                },
            )
        }
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_subscribe(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsSubscribeParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let job_id = match job_id(&params.job_id) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    let session_id = match meerkat_core::SessionId::parse(&params.session_id) {
        Ok(session_id) => session_id,
        Err(error) => {
            return RpcResponse::error(id, error::INVALID_PARAMS, error.to_string());
        }
    };
    let delivery = match params.delivery {
        meerkat_contracts::JobDeliveryKind::Record => JobDeliveryKind::Record,
        meerkat_contracts::JobDeliveryKind::Notification => JobDeliveryKind::Notification,
        meerkat_contracts::JobDeliveryKind::Event { handling_mode } => JobDeliveryKind::Event {
            handling_mode: handling_mode.into(),
        },
    };
    let subscription_id = match JobSubscriptionId::new(params.subscription_id) {
        Ok(value) => value,
        Err(error) => return response_error(id, error),
    };
    let service = runtime.detached_job_service();
    if let Err(error) = described(runtime, &job_id).await {
        return response_error(id, error);
    }
    match service
        .subscribe(
            &job_id,
            JobSubscription::new(subscription_id, session_id, delivery),
        )
        .await
    {
        Ok(_) => match projected(runtime, &job_id).await {
            Ok(job) => RpcResponse::success(id, JobsSubscribeResult { job }),
            Err(error) => response_error(id, error),
        },
        Err(error) => response_error(id, error),
    }
}

pub async fn handle_unsubscribe(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
) -> RpcResponse {
    let params: JobsUnsubscribeParams = match parse(id.clone(), params) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let job_id = match job_id(&params.job_id) {
        Ok(job_id) => job_id,
        Err(error) => return response_error(id, error),
    };
    let subscription_id = match JobSubscriptionId::new(params.subscription_id) {
        Ok(value) => value,
        Err(error) => return response_error(id, error),
    };
    let service = runtime.detached_job_service();
    if let Err(error) = described(runtime, &job_id).await {
        return response_error(id, error);
    }
    match service.unsubscribe(&job_id, &subscription_id).await {
        Ok(_) => match projected(runtime, &job_id).await {
            Ok(job) => RpcResponse::success(id, JobsUnsubscribeResult { job }),
            Err(error) => response_error(id, error),
        },
        Err(error) => response_error(id, error),
    }
}

macro_rules! host_mutation {
    ($name:ident, $params:ty, |$service:ident, $job_id:ident, $write:ident, $value:ident| $body:expr) => {
        pub async fn $name(
            id: Option<RpcId>,
            params: Option<&RawValue>,
            runtime: &Arc<SessionRuntime>,
        ) -> RpcResponse {
            let $value: $params = match parse(id.clone(), params) {
                Ok(params) => params,
                Err(response) => return response,
            };
            let ($job_id, $write) = match write_authority($value.authority.clone()) {
                Ok(authority) => authority,
                Err(error) => return response_error(id, error),
            };
            let $service = runtime.detached_job_service();
            if let Err(error) = described(runtime, &$job_id).await {
                return response_error(id, error);
            }
            let mutation: Result<_, DetachedJobError> = async { $body.await }.await;
            match mutation {
                Ok(_) => match projected(runtime, &$job_id).await {
                    Ok(job) => {
                        let delivery_runtime = Arc::clone(runtime);
                        tokio::spawn(async move {
                            if let Err(error) = delivery_runtime.drain_job_deliveries().await {
                                tracing::warn!(%error, "post-mutation durable job delivery drain failed");
                            }
                        });
                        RpcResponse::success(id, MobkitJobMutationResult { job })
                    }
                    Err(error) => response_error(id, error),
                },
                Err(error) => response_error(id, error),
            }
        }
    };
}

host_mutation!(
    handle_heartbeat,
    MobkitJobHeartbeatParams,
    |service, job_id, write, value| service.renew_lease(
        &job_id,
        write,
        value.heartbeat_at_ms,
        value.lease_expires_at_ms
    )
);
host_mutation!(
    handle_progress,
    MobkitJobProgressParams,
    |service, job_id, write, value| service.report_progress(
        &job_id,
        write,
        JobProgress::new(value.cursor, value.detail)?,
        value.observed_at_ms
    )
);
host_mutation!(
    handle_checkpoint,
    MobkitJobCheckpointParams,
    |service, job_id, write, value| service.record_checkpoint(
        &job_id,
        write,
        CheckpointRef::new(value.checkpoint_ref)?,
        value.observed_at_ms
    )
);
host_mutation!(
    handle_complete,
    MobkitJobCompleteParams,
    |service, job_id, write, value| service.complete_attempt(
        &job_id,
        write,
        value.completed_at_ms,
        value.result_ref.map(JobResultRef::new).transpose()?
    )
);
host_mutation!(
    handle_fail,
    MobkitJobFailParams,
    |service, job_id, write, value| service.fail_attempt(
        &job_id,
        write,
        value.failed_at_ms,
        JobFailureCode::new(value.code)?,
        value.detail_ref.map(JobResultRef::new).transpose()?
    )
);
host_mutation!(
    handle_cancel_ack,
    MobkitJobCancelAckParams,
    |service, job_id, write, value| service.acknowledge_cancel(
        &job_id,
        write,
        value.acknowledged_at_ms
    )
);
