//! Cross-target fixture for the production member-status observation deadline.
//!
//! Only the session service is controlled here. Deadline ownership, view-read
//! classification, single flight, and draining use the production actor path.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    SessionControlError, SessionError, SessionHistoryPage, SessionHistoryQuery, SessionQuery,
    SessionService, SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt,
    SessionSummary, SessionView, StartTurnRequest,
};
use meerkat_core::time_compat::Duration;
use meerkat_core::{AgentExecutionSnapshot, RunResult, SessionId};

use super::actor::MobActor;
use super::actor::member_status_lane::{MemberStatusViewReadDrainOutcome, MemberStatusViewReads};
use super::handle::MemberPreviewUnavailable;
use super::session_service::{
    MemberStatusSessionView, MobSessionService, ResumeSessionLoad, SessionResumeAuthority,
    SessionResumeVerdict,
};
use crate::AgentIdentity;
use crate::tokio;

/// Evidence returned by the actual observation and drain owners.
#[doc(hidden)]
#[derive(Debug)]
pub struct MemberStatusDeadlineTestObservation {
    pub preview_unavailable: Option<MemberPreviewUnavailable>,
    pub output_preview: Option<String>,
    pub tokens_used: u64,
    pub snapshot_calls: usize,
    pub status_view_calls: usize,
    pub reads_before_drain: usize,
    pub retained_drain: bool,
    pub drain_published: bool,
    pub reads_after_drain: usize,
}

/// Exercise the whole observation budget after its preliminary snapshot waits.
///
/// The snapshot never answers, consuming its production 250 ms timeout. The
/// view answers 875 ms after it is first polled, between the correct remaining
/// 750 ms budget and an incorrectly restarted 1 s budget. The 125 ms margins
/// avoid depending on exact timer callback ordering. Any retained read is
/// drained before returning, so the fixture leaves no detached work behind.
#[doc(hidden)]
pub async fn member_status_deadline_after_snapshot_wait_for_test()
-> MemberStatusDeadlineTestObservation {
    let service = Arc::new(DelayedStatusService::default());
    let view_reads = MemberStatusViewReads::default();
    let (observation, drain) = MobActor::observe_member_status_session(
        service.clone(),
        None,
        AgentIdentity::from("member-status-deadline-fixture"),
        Some(SessionId::new()),
        true,
        0,
        &view_reads,
    )
    .await;
    let reads_before_drain = view_reads.in_flight();
    let retained_drain = drain.is_some();
    let drain_published = match drain {
        Some(drain) => matches!(
            drain.finish(Duration::from_secs(2)).await,
            MemberStatusViewReadDrainOutcome::Published
        ),
        None => false,
    };
    MemberStatusDeadlineTestObservation {
        preview_unavailable: observation.preview_unavailable,
        output_preview: observation.output_preview,
        tokens_used: observation.tokens_used,
        snapshot_calls: service.snapshot_calls.load(Ordering::SeqCst),
        status_view_calls: service.status_view_calls.load(Ordering::SeqCst),
        reads_before_drain,
        retained_drain,
        drain_published,
        reads_after_drain: view_reads.in_flight(),
    }
}

#[derive(Default)]
struct DelayedStatusService {
    snapshot_calls: AtomicUsize,
    status_view_calls: AtomicUsize,
}

fn unsupported() -> SessionError {
    SessionError::Unsupported("member-status deadline fixture only supports observation".into())
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl SessionService for DelayedStatusService {
    async fn create_session(&self, _req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        Err(unsupported())
    }

    async fn start_turn(
        &self,
        _id: &SessionId,
        _req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        Err(unsupported())
    }

    async fn interrupt(&self, _id: &SessionId) -> Result<(), SessionError> {
        Err(unsupported())
    }

    async fn read(&self, _id: &SessionId) -> Result<SessionView, SessionError> {
        Err(unsupported())
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        Err(unsupported())
    }

    async fn archive(&self, _id: &SessionId) -> Result<(), SessionError> {
        Err(unsupported())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl SessionServiceCommsExt for DelayedStatusService {}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl SessionServiceControlExt for DelayedStatusService {
    async fn append_system_context(
        &self,
        _id: &SessionId,
        _req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        Err(unsupported().into())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl SessionServiceHistoryExt for DelayedStatusService {
    async fn read_history(
        &self,
        _id: &SessionId,
        _query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        Err(unsupported())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl MobSessionService for DelayedStatusService {
    async fn execution_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        self.snapshot_calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }

    async fn observe_member_status_view(
        &self,
        _session_id: &SessionId,
    ) -> Result<MemberStatusSessionView, SessionError> {
        self.status_view_calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(875)).await;
        Ok(MemberStatusSessionView::live_watch(
            Some("view completed after the whole-observation deadline".into()),
            17,
        ))
    }

    async fn observe_live_durable_source(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::LiveDurableSourceObservation, SessionError> {
        crate::observe_live_durable_source_via_projection_visibility(self, session_id).await
    }

    async fn commit_live_delegation_final_transcript(
        &self,
        _machine: &meerkat_runtime::MeerkatMachine,
        _session_id: &SessionId,
        _provisional: meerkat_core::ProvisionalLiveHandoff,
        _final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        Err(unsupported())
    }

    async fn commit_live_delegation_final_transcript_at_turn_boundary(
        &self,
        _machine: &meerkat_runtime::MeerkatMachine,
        _session_id: &SessionId,
        _provisional: meerkat_core::ProvisionalLiveHandoff,
        _final_event: meerkat_core::RealtimeTranscriptEvent,
        _bound: Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        Err(unsupported())
    }

    async fn create_session_under_runtime_turn_boundary(
        &self,
        _req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        Err(unsupported())
    }

    async fn fork_persisted_session_at_turn_boundary(
        &self,
        _source_session_id: &SessionId,
        _message_count: Option<usize>,
        _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        _target: meerkat_core::DurableSessionForkTarget,
        _bound: Duration,
    ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
        Err(unsupported())
    }

    async fn load_session_for_resume(
        &self,
        _session_id: &SessionId,
    ) -> Result<ResumeSessionLoad, SessionError> {
        Err(unsupported())
    }

    async fn observe_session_resume_authority(
        &self,
        _session_id: &SessionId,
    ) -> Result<SessionResumeAuthority, SessionError> {
        Err(unsupported())
    }

    async fn materialize_session_resume_verdict(
        &self,
        _session_id: &SessionId,
    ) -> Result<SessionResumeVerdict, SessionError> {
        Err(unsupported())
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), SessionError> {
        Err(unsupported())
    }

    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        _session_id: &SessionId,
        _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        Err(unsupported())
    }

    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        _session_id: &SessionId,
        _runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        Err(unsupported())
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), SessionError> {
        Err(unsupported())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::tokio;

    #[tokio::test]
    async fn member_status_deadline_includes_preliminary_execution_snapshot_wait() {
        let observed = member_status_deadline_after_snapshot_wait_for_test().await;
        assert_eq!(
            observed.preview_unavailable,
            Some(MemberPreviewUnavailable::ObservationDeadline),
            "the snapshot wait must consume the same deadline as the view read: {observed:?}"
        );
        assert_eq!(observed.output_preview, None);
        assert_eq!(observed.tokens_used, 0);
        assert_eq!(observed.snapshot_calls, 1);
        assert_eq!(observed.status_view_calls, 1);
        assert_eq!(observed.reads_before_drain, 1);
        assert!(observed.retained_drain);
        assert!(observed.drain_published);
        assert_eq!(observed.reads_after_drain, 0);
    }
}
