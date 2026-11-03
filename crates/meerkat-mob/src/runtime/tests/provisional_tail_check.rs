//! The durable tail presence check a resume spawn asks before anything could
//! settle a tail fails closed: a session service that reports persistent
//! sessions but does not forward it refuses, typed, and the spawn with it.

use super::*;

use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, SessionControlError, SessionHistoryPage,
    SessionHistoryQuery, SessionQuery, SessionServiceCommsExt, SessionServiceControlExt,
    SessionServiceHistoryExt, SessionSummary, SessionView, StartTurnRequest,
};
use meerkat_core::time_compat::Duration;

use crate::runtime::session_service::{
    MemberStatusSessionView, ResumeSessionLoad, SessionResumeAuthority, SessionResumeVerdict,
};

/// A session service that overrides nothing but its persistence report: the
/// durable tail presence check is the trait default.
struct UnforwardedTailCheckService {
    persistent: bool,
}

fn unused() -> SessionError {
    SessionError::Unsupported("tail presence check fixture".into())
}

#[async_trait::async_trait]
impl SessionService for UnforwardedTailCheckService {
    async fn create_session(&self, _req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        Err(unused())
    }

    async fn start_turn(
        &self,
        _id: &SessionId,
        _req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        Err(unused())
    }

    async fn interrupt(&self, _id: &SessionId) -> Result<(), SessionError> {
        Err(unused())
    }

    async fn read(&self, _id: &SessionId) -> Result<SessionView, SessionError> {
        Err(unused())
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        Err(unused())
    }

    async fn archive(&self, _id: &SessionId) -> Result<(), SessionError> {
        Err(unused())
    }
}

#[async_trait::async_trait]
impl SessionServiceCommsExt for UnforwardedTailCheckService {}

#[async_trait::async_trait]
impl SessionServiceControlExt for UnforwardedTailCheckService {
    async fn append_system_context(
        &self,
        _id: &SessionId,
        _req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        Err(unused().into())
    }
}

#[async_trait::async_trait]
impl SessionServiceHistoryExt for UnforwardedTailCheckService {
    async fn read_history(
        &self,
        _id: &SessionId,
        _query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        Err(unused())
    }
}

#[async_trait::async_trait]
impl MobSessionService for UnforwardedTailCheckService {
    fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
        meerkat_core::SessionCommsIdentityRoot::NotKept
    }

    async fn subscribe_session_activity(
        &self,
        _session_id: &SessionId,
    ) -> Result<crate::MemberSessionActivity, SessionError> {
        Ok(crate::MemberSessionActivity::inactive())
    }

    async fn observe_member_status_view(
        &self,
        _session_id: &SessionId,
    ) -> Result<MemberStatusSessionView, SessionError> {
        Err(unused())
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
        Err(unused())
    }

    async fn commit_live_delegation_final_transcript_at_turn_boundary(
        &self,
        _machine: &meerkat_runtime::MeerkatMachine,
        _session_id: &SessionId,
        _provisional: meerkat_core::ProvisionalLiveHandoff,
        _final_event: meerkat_core::RealtimeTranscriptEvent,
        _bound: Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        Err(unused())
    }

    async fn create_session_under_runtime_turn_boundary(
        &self,
        _req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        Err(unused())
    }

    async fn fork_persisted_session_at_turn_boundary(
        &self,
        _source_session_id: &SessionId,
        _message_count: Option<usize>,
        _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        _target: meerkat_core::DurableSessionForkTarget,
        _bound: Duration,
    ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
        Err(unused())
    }

    async fn load_session_for_resume(
        &self,
        _session_id: &SessionId,
    ) -> Result<ResumeSessionLoad, SessionError> {
        Err(unused())
    }

    async fn observe_session_resume_authority(
        &self,
        _session_id: &SessionId,
    ) -> Result<SessionResumeAuthority, SessionError> {
        Err(unused())
    }

    async fn materialize_session_resume_verdict(
        &self,
        _session_id: &SessionId,
    ) -> Result<SessionResumeVerdict, SessionError> {
        Err(unused())
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), SessionError> {
        Err(unused())
    }

    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        _session_id: &SessionId,
        _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        Err(unused())
    }

    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        _session_id: &SessionId,
        _runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        Err(unused())
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), SessionError> {
        Err(unused())
    }

    fn supports_persistent_sessions(&self) -> bool {
        self.persistent
    }
}

/// The trait default of the durable tail presence check fails closed. A
/// service that reports persistent sessions and does not override it refuses
/// the check, typed, naming the session and the method to forward; a service
/// that keeps no durable sessions answers that no tail stands.
#[tokio::test]
async fn the_tail_presence_check_fails_closed_for_a_persistent_service_that_does_not_forward_it() {
    let session_id = SessionId::new();
    match (UnforwardedTailCheckService { persistent: true })
        .provisional_tail_stands(&session_id)
        .await
    {
        Err(SessionError::Unsupported(reason)) => {
            assert!(
                reason.contains("MobSessionService::provisional_tail_stands"),
                "the refusal names the method to forward: {reason}"
            );
            assert!(
                reason.contains(&session_id.to_string()),
                "the refusal names the session: {reason}"
            );
        }
        other => panic!(
            "a persistent service that does not forward the check must refuse it typed: \
             {other:?}"
        ),
    }
    assert!(
        matches!(
            (UnforwardedTailCheckService { persistent: false })
                .provisional_tail_stands(&session_id)
                .await,
            Ok(false)
        ),
        "a service that keeps no durable sessions holds no tail"
    );
}

/// The trait default of the cold-predecessor settlement refuses, typed, and
/// touches nothing: only a runtime-backed persistent service settles, so a
/// wrapper must forward the call.
#[tokio::test]
async fn the_cold_predecessor_settlement_default_refuses_unsupported() {
    let session_id = SessionId::new();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let claim = machine
        .grant_session_hosting(&session_id)
        .expect("a storeless machine grants its own claim");
    let refused = (UnforwardedTailCheckService { persistent: true })
        .settle_cold_predecessor_with_mob_lifecycle_authority_before(
            &session_id,
            &claim,
            meerkat_core::time_compat::Instant::now() + Duration::from_secs(1),
        )
        .await
        .expect_err("the default settles nothing");
    assert_eq!(refused.session_id, session_id);
    assert!(
        matches!(
            refused.refusal,
            meerkat_session::ColdPredecessorSettleRefusal::Session(SessionError::Unsupported(_))
        ),
        "{refused:?}"
    );
    assert!(refused.progress.tail.is_none());
    assert!(refused.progress.inputs.is_empty());
    assert!(refused.progress.owner_loss.is_none());
}

/// A resume spawn through a persistent session service that does not forward
/// the durable tail presence check is refused with that typed refusal before
/// anything reads or settles the session: the check fails closed, it is not
/// skipped. The same spawn with the check forwarded restores the session.
#[tokio::test]
async fn a_resume_spawn_through_a_persistent_service_that_does_not_forward_the_tail_check_refuses()
{
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let initial = service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "seed".to_string().into(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: Some(4096),
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                comms_name: Some(format!("{}/worker/w-resume", test_mob_id())),
                ..Default::default()
            }),
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        })
        .await
        .expect("seed session");
    let session_id = initial.session_id.clone();
    MobSessionService::discard_live_session(service.as_ref(), &session_id)
        .await
        .expect("discard live session");
    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    service.stop_forwarding_provisional_tail_check();
    let refused = handle
        .spawn_spec(
            SpawnMemberSpec::new("worker", "w-resume")
                .with_resume_bridge_session_id(session_id.clone()),
        )
        .await;
    match &refused {
        Err(MobError::SessionError(SessionError::Unsupported(reason))) => assert!(
            reason.contains("MobSessionService::provisional_tail_stands"),
            "the spawn surfaces the unforwarded check: {reason}"
        ),
        other => panic!(
            "a resume spawn through a persistent service that does not forward the tail \
             check must refuse with it: {:?}",
            other.as_ref().err().map(ToString::to_string)
        ),
    }
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-resume"))
            .await
            .expect("read the roster")
            .is_none(),
        "the refused spawn rosters no member"
    );

    service
        .forwards_provisional_tail_check
        .store(true, Ordering::SeqCst);
    handle
        .spawn_spec(
            SpawnMemberSpec::new("worker", "w-resume")
                .with_resume_bridge_session_id(session_id.clone()),
        )
        .await
        .expect("with the check forwarded, the resume spawn restores the session");
    assert_eq!(
        handle
            .resolve_bridge_session_id(&AgentIdentity::from("w-resume"))
            .await
            .as_ref(),
        Some(&session_id),
        "the restored member keeps the requested session"
    );
}
