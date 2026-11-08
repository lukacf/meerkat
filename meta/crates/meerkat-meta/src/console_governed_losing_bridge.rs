//! The real mob session bridge for the Console's governed bind tests, except
//! that a governed delivery's answer is lost: after a real native admission
//! (`PostAdmissionInternal`), or before native admission settled anything
//! (`CustodyLost`). A copy of the governed ingress test's losing bridge, for
//! the Console's own send path.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::types::HandlingMode;
use meerkat_mob::MobError;
use meerkat_mob::ids::AgentIdentity as MobAgentIdentity;

use crate::actor_loop_health::ActorLoopHealth;
use crate::identity_first::*;

/// How a governed delivery's answer is lost.
#[derive(Clone, Copy)]
pub(super) enum LostAnswer {
    /// Native work failed with an internal error after admission.
    PostAdmissionInternal,
    /// The submission and its native custody were lost before native
    /// admission settled it.
    CustodyLost,
}

/// The real mob session bridge, except that after the real governed
/// delivery ran its answer is replaced by an unresolved one.
pub(super) struct LosingBridge {
    pub(super) inner: MobSessionBridge,
    pub(super) lose: LostAnswer,
    /// Real native acknowledgements of new work seen before an answer
    /// was replaced.
    pub(super) native_accepted: Arc<AtomicUsize>,
}

#[async_trait]
impl SessionBridge for LosingBridge {
    async fn begin_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        if matches!(self.lose, LostAnswer::CustodyLost) {
            // Taken past its guard and lost unsettled: no native
            // settlement is ever observed.
            drop((input, custody.submit()));
            return Err(BridgeAdmissionError::GovernedInput(
                GovernedInputError::Mob(Box::new(MobError::Internal(
                    "the submission was lost".to_string(),
                ))),
            ));
        }
        // The real native admission runs, settles the custody and must
        // actually accept the input as new work; only then is its answer
        // replaced, so a native refusal can never pass for a lost answer.
        let real = self
            .inner
            .begin_governed_delivery(runtime_id, session_id, input, custody)
            .await;
        if matches!(
            &real,
            Ok(GovernedDelivery { admission: Some(admission), .. })
                if !admission.deduplicated
        ) {
            self.native_accepted.fetch_add(1, Ordering::SeqCst);
        }
        Err(BridgeAdmissionError::GovernedInput(
            GovernedInputError::Mob(Box::new(MobError::Internal(
                "a stage failed after admission".to_string(),
            ))),
        ))
    }

    fn observe_actor_loop_health(&self, health: Arc<ActorLoopHealth>) {
        self.inner.observe_actor_loop_health(health);
    }

    async fn reload_member_registration(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<Option<BridgeMemberReload>, BridgeError> {
        self.inner.reload_member_registration(runtime_id).await
    }

    async fn release_member_run_starts_for_published_tools(
        &self,
        member_id: &MobAgentIdentity,
    ) -> Result<(), BridgeError> {
        self.inner
            .release_member_run_starts_for_published_tools(member_id)
            .await
    }

    fn members_holding_run_starts_for_published_tools(
        &self,
    ) -> Result<Vec<MobAgentIdentity>, BridgeError> {
        self.inner.members_holding_run_starts_for_published_tools()
    }

    fn member_holds_run_starts_for_published_tools(
        &self,
        member_id: &MobAgentIdentity,
    ) -> Result<bool, BridgeError> {
        self.inner
            .member_holds_run_starts_for_published_tools(member_id)
    }

    async fn resolve_member_build_preparation(
        &self,
        member_id: &MobAgentIdentity,
    ) -> Result<(), BridgeError> {
        self.inner.resolve_member_build_preparation(member_id).await
    }

    async fn member_durability(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<MemberDurability> {
        self.inner.member_durability(session_id).await
    }

    async fn raw_member_alias_exists(&self, alias: &str) -> Result<bool, BridgeError> {
        self.inner.raw_member_alias_exists(alias).await
    }

    async fn recover_committed_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<CommittedBoundaryRepair, BridgeError> {
        self.inner.recover_committed_boundary(session_id).await
    }

    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.inner.create_session(identity, spec, draft).await
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        self.inner
            .resume_session(identity, spec, draft, session_id, resume_from_role)
            .await
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.inner.deliver_admitted(runtime_id, delivery).await
    }

    async fn deliver_host_human_input(
        &self,
        runtime_id: &AgentRuntimeId,
        expected_session: &meerkat_core::SessionId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::SessionId, BridgeError> {
        self.inner
            .deliver_host_human_input(runtime_id, expected_session, delivery)
            .await
    }

    async fn begin_awaiting_commit(
        &self,
        runtime_id: &AgentRuntimeId,
        content: &meerkat_core::ContentInput,
        system_prompt: Option<&str>,
        injected_context: &[meerkat_core::ContentInput],
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        self.inner
            .begin_awaiting_commit(
                runtime_id,
                content,
                system_prompt,
                injected_context,
                handling_mode,
                interaction_id,
            )
            .await
    }

    fn tracks_turn_output(&self) -> bool {
        self.inner.tracks_turn_output()
    }

    async fn governed_controller(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::ControllerModelClient, BridgeAdmissionError> {
        self.inner.governed_controller(runtime_id, session_id).await
    }

    async fn begin_delivery_with_output(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        self.inner
            .begin_delivery_with_output(runtime_id, delivery)
            .await
    }

    async fn checkpoint_session(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        self.inner.checkpoint_session(runtime_id, session_id).await
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.inner.retire_member(runtime_id).await
    }

    async fn retire_reset_superseded_member(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        self.inner
            .retire_reset_superseded_member(runtime_id, session_id)
            .await
    }

    async fn wire_peer(&self, a: &AgentRuntimeId, b: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.inner.wire_peer(a, b).await
    }

    async fn wire_peers_batch(
        &self,
        edges: &[(AgentRuntimeId, AgentRuntimeId)],
    ) -> Result<(), BridgeError> {
        self.inner.wire_peers_batch(edges).await
    }

    async fn current_member_wires(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        self.inner.current_member_wires().await
    }

    async fn current_member_wires_any_half(
        &self,
    ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
        self.inner.current_member_wires_any_half().await
    }

    async fn unwire_peer(&self, a: &AgentRuntimeId, b: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.inner.unwire_peer(a, b).await
    }

    async fn inspect_member(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<MemberInspection, BridgeError> {
        self.inner.inspect_member(runtime_id).await
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.inner.live_member(identity).await
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.inner.live_member_scope(identity).await
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.inner.ensure_member_live(identity).await
    }

    async fn identity_convergence_status(
        &self,
        identity: &AgentIdentity,
    ) -> Result<
        meerkat_mob::identity::IdentityStoredObservation<
            meerkat_mob::identity::IdentityConvergenceStatus,
        >,
        BridgeError,
    > {
        self.inner.identity_convergence_status(identity).await
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.inner.member_binding_history().await
    }

    async fn identity_entry(
        &self,
        identity: &AgentIdentity,
    ) -> Result<CoreIdentityEntry, BridgeError> {
        SessionBridge::identity_entry(&self.inner, identity).await
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.inner.last_member_binding(identity).await
    }

    async fn identity_management(
        &self,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        self.inner.identity_management(identity).await
    }

    async fn durable_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        self.inner.durable_identity_members().await
    }

    async fn durable_session_held(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        self.inner.durable_session_held(session_id).await
    }

    async fn register_session_runtime_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
        identity: &AgentIdentity,
        generation: ContinuityGeneration,
        checkpoint_version: CheckpointVersion,
        fencing_token: FencingToken,
    ) -> Result<CheckpointVersion, BridgeError> {
        self.inner
            .register_session_runtime_state(
                session_id,
                identity,
                generation,
                checkpoint_version,
                fencing_token,
            )
            .await
    }

    async fn suspend_session_runtime_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        self.inner.suspend_session_runtime_state(session_id).await
    }

    async fn unregister_session_runtime_state(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), BridgeError> {
        self.inner
            .unregister_session_runtime_state(session_id)
            .await
    }

    async fn apply_declared_roster(
        &self,
        roster: DeclaredRosterRequest,
    ) -> Result<DeclaredRosterAnswer, BridgeError> {
        self.inner.apply_declared_roster(roster).await
    }
}
