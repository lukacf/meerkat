//! Linear-type spawn guard for provisioned members.
//!
//! [`PendingProvision`] wraps a provisioned [`MemberRef`] and enforces at the
//! type level that the resource is either **committed** (added to the roster)
//! or **rolled back** (session archived). Dropping without consuming is a bug
//! and always panics.
//!
//! The `#[must_use]` attribute on the type ensures the compiler warns if the
//! value is discarded.

use super::provisioner::{MobProvisioner, ProvisionSessionOrigin, ResumedMemberRollbackAuthority};
use crate::error::MobError;
use crate::event::MemberRef;
use crate::ids::AgentIdentity;
use std::sync::Arc;

/// A provisioned member that must be explicitly committed or rolled back.
///
/// Dropping without consuming is a bug and always panics. The async
/// `rollback()` method performs the actual cleanup;
/// the synchronous `Drop` can only detect the mistake, not fix it.
#[must_use = "provisioned member must be committed or rolled back"]
pub(super) struct PendingProvision {
    member_ref: Option<MemberRef>,
    agent_identity: AgentIdentity,
    provisioner: Arc<dyn MobProvisioner>,
    operation_id: meerkat_core::ops::OperationId,
    session_origin: ProvisionSessionOrigin,
    rollback_authority: Option<ResumedMemberRollbackAuthority>,
    committed: bool,
    rollback_attempted: bool,
}

impl PendingProvision {
    pub(super) fn new(
        member_ref: MemberRef,
        agent_identity: AgentIdentity,
        provisioner: Arc<dyn MobProvisioner>,
        operation_id: meerkat_core::ops::OperationId,
        session_origin: ProvisionSessionOrigin,
        rollback_authority: Option<ResumedMemberRollbackAuthority>,
    ) -> Self {
        Self {
            member_ref: Some(member_ref),
            agent_identity,
            provisioner,
            operation_id,
            session_origin,
            rollback_authority,
            committed: false,
            rollback_attempted: false,
        }
    }

    /// Consume the provision, returning the member ref for roster insertion.
    ///
    /// Returns `Err` only if called after prior consumption (structurally
    /// impossible since `commit` and `rollback` both take `self` by value).
    pub(super) fn commit(mut self) -> Result<MemberRef, MobError> {
        self.committed = true;
        self.take_member_ref("commit")
    }

    /// Consume the provision for a FAILED host-materialized spawn without
    /// local retire traffic: the member on the host is a documented orphan
    /// seed reclaimed by the HostStatus sweep at stale fence (§4.4), and the
    /// member ref addresses a remote peer a local `retire_member` must not
    /// dial. The caller aborts the local ops-provision record separately.
    pub(super) fn disarm_remote_orphan_seed(mut self) -> Result<MemberRef, MobError> {
        self.committed = true;
        self.take_member_ref("disarm_remote_orphan_seed")
    }

    /// Roll back the provision. Fresh sessions are retired/archived; resumed
    /// durable sessions are detached and restored to durable idle.
    pub(super) async fn rollback(mut self) -> Result<(), MobError> {
        let member_ref = self.take_member_ref("rollback")?;
        let rollback = match self.session_origin {
            ProvisionSessionOrigin::Fresh => self
                .provisioner
                .retire_member(&member_ref)
                .await
                .map(|_| ()),
            ProvisionSessionOrigin::ResumedDurable | ProvisionSessionOrigin::RevivedRetired => {
                let rollback_authority = self.rollback_authority.as_ref().ok_or_else(|| {
                    MobError::Internal(format!(
                        "resumed provision '{}' lost its exact rollback attachment authority",
                        self.agent_identity
                    ))
                })?;
                self.provisioner
                    .restore_resumed_member(
                        &member_ref,
                        &self.operation_id,
                        self.session_origin,
                        rollback_authority,
                    )
                    .await
            }
        };
        match rollback {
            Ok(()) => {
                self.committed = true;
                Ok(())
            }
            Err(error) => {
                self.rollback_attempted = true;
                self.member_ref = Some(member_ref);
                Err(error)
            }
        }
    }

    /// Access the member ref without consuming.
    pub(super) fn member_ref(&self) -> Result<&MemberRef, MobError> {
        // `member_ref` is `Some` from construction until `commit()` or
        // `rollback()` consume `self`. Borrowing through `&self` cannot happen
        // after consumption.
        self.member_ref.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "PendingProvision::member_ref called after prior consumption for '{}'",
                self.agent_identity
            ))
        })
    }

    /// The meerkat ID associated with this provision.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn member_identity(&self) -> &AgentIdentity {
        &self.agent_identity
    }

    pub(super) fn session_origin(&self) -> ProvisionSessionOrigin {
        self.session_origin
    }

    /// Extract the member ref, returning an error if already consumed.
    fn take_member_ref(&mut self, caller: &str) -> Result<MemberRef, MobError> {
        self.member_ref.take().ok_or_else(|| {
            MobError::Internal(format!(
                "PendingProvision::{caller} called after prior consumption for '{}'",
                self.agent_identity,
            ))
        })
    }
}

impl Drop for PendingProvision {
    fn drop(&mut self) {
        if !self.committed && !self.rollback_attempted {
            let member_ref = self.member_ref.take();
            tracing::error!(
                agent_identity = %self.agent_identity,
                member_ref = ?member_ref,
                "PendingProvision dropped without commit or rollback — resource leak"
            );
            debug_assert!(false, "PendingProvision dropped without commit or rollback");
        } else if self.rollback_attempted {
            tracing::error!(
                agent_identity = %self.agent_identity,
                member_ref = ?self.member_ref,
                "PendingProvision rollback was attempted but failed"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ids::AgentIdentity;
    use crate::runtime::handle::MemberSpawnReceipt;
    use crate::runtime::provisioner::ProvisionMemberRequest;
    use async_trait::async_trait;
    use meerkat_core::comms::TrustedPeerDescriptor;
    use meerkat_core::event_injector::SubscribableInjector;
    use meerkat_core::ops::OperationId;
    use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
    use meerkat_core::service::StartTurnRequest;
    use meerkat_core::types::SessionId;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct MockProvisioner {
        retired: AtomicBool,
        restored: AtomicBool,
        started: AtomicBool,
        fail_retire: bool,
    }

    impl MockProvisioner {
        fn new() -> Self {
            Self {
                retired: AtomicBool::new(false),
                restored: AtomicBool::new(false),
                started: AtomicBool::new(false),
                fail_retire: false,
            }
        }

        fn failing_retire() -> Self {
            Self {
                retired: AtomicBool::new(false),
                restored: AtomicBool::new(false),
                started: AtomicBool::new(false),
                fail_retire: true,
            }
        }
    }

    #[async_trait]
    impl MobProvisioner for MockProvisioner {
        async fn provision_member(
            &self,
            _req: ProvisionMemberRequest,
        ) -> Result<MemberSpawnReceipt, MobError> {
            Ok(MemberSpawnReceipt {
                member_ref: MemberRef::from_bridge_session_id(SessionId::new()),
                operation_id: meerkat_core::ops::OperationId::new(),
                session_origin: ProvisionSessionOrigin::Fresh,
                rollback_authority: None,
                materialized_ack: None,
                failed_restore_peer_ids: Vec::new(),
            })
        }

        async fn abort_member_provision(
            &self,
            _member_ref: &MemberRef,
            _operation_id: &OperationId,
            _reason: &str,
        ) -> Result<(), MobError> {
            self.retired.store(true, Ordering::Release);
            Ok(())
        }

        async fn capture_resumed_member_rollback_authority(
            &self,
            _member_ref: &MemberRef,
        ) -> Result<ResumedMemberRollbackAuthority, MobError> {
            Ok(ResumedMemberRollbackAuthority::for_test())
        }

        async fn restore_resumed_member(
            &self,
            _member_ref: &MemberRef,
            _operation_id: &OperationId,
            _original_origin: ProvisionSessionOrigin,
            _rollback_authority: &ResumedMemberRollbackAuthority,
        ) -> Result<(), MobError> {
            self.restored.store(true, Ordering::Release);
            Ok(())
        }

        async fn retire_member(
            &self,
            _member_ref: &MemberRef,
        ) -> Result<crate::machines::mob_machine::MemberSessionDisposal, MobError> {
            self.retired.store(true, Ordering::Release);
            if self.fail_retire {
                return Err(MobError::Internal("retire failed".to_string()));
            }
            Ok(crate::machines::mob_machine::MemberSessionDisposal::Archived)
        }

        async fn interrupt_member(
            &self,
            _member_ref: &MemberRef,
            _expected_member: Option<&super::super::bridge_protocol::BridgeMemberIncarnation>,
        ) -> Result<(), MobError> {
            Ok(())
        }

        async fn hard_cancel_member(
            &self,
            _member_ref: &MemberRef,
            _reason: &str,
        ) -> Result<(), MobError> {
            Ok(())
        }

        async fn start_turn(
            &self,
            _member_ref: &MemberRef,
            _req: StartTurnRequest,
        ) -> Result<(), MobError> {
            self.started.store(true, Ordering::Release);
            Ok(())
        }

        async fn admit_turn(
            &self,
            _member_ref: &MemberRef,
            _req: StartTurnRequest,
        ) -> Result<(), MobError> {
            Ok(())
        }

        async fn interaction_event_injector(
            &self,
            _session_id: &SessionId,
        ) -> Option<Arc<dyn SubscribableInjector>> {
            None
        }

        async fn is_member_active(
            &self,
            _member_ref: &MemberRef,
        ) -> Result<Option<bool>, MobError> {
            Ok(None)
        }

        async fn comms_runtime(
            &self,
            _member_ref: &MemberRef,
        ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
            None
        }

        async fn trusted_peer_spec(
            &self,
            _member_ref: &MemberRef,
            _fallback_name: &str,
            _fallback_peer_id: &str,
        ) -> Result<TrustedPeerDescriptor, MobError> {
            Err(MobError::Internal("not implemented".into()))
        }

        async fn publish_trusted_peer_spec_for_operation(
            &self,
            _member_ref: &MemberRef,
            _operation_id: &OperationId,
            _trusted_peer: TrustedPeerDescriptor,
        ) -> Result<(), MobError> {
            Ok(())
        }

        async fn active_operation_id_for_member(
            &self,
            _member_ref: &MemberRef,
        ) -> Option<OperationId> {
            None
        }

        async fn bind_member_owner_context(
            &self,
            _member_ref: &MemberRef,
            _owner_bridge_session_id: SessionId,
            _ops_registry: Arc<dyn OpsLifecycleRegistry>,
        ) -> Result<(), MobError> {
            Ok(())
        }
    }

    fn start_turn_request() -> StartTurnRequest {
        StartTurnRequest {
            injected_context: Vec::new(),
            prompt: meerkat_core::types::ContentInput::Text("work".to_string()),
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
        }
    }

    #[tokio::test]
    async fn default_correlation_lane_rejects_placed_context_without_starting() {
        let provisioner = MockProvisioner::new();
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let error = provisioner
            .start_turn_with_correlation(
                &member_ref,
                start_turn_request(),
                Some(crate::runtime::provisioner::PlacedTurnDeliveryContext {
                    input_id: uuid::Uuid::new_v4().to_string(),
                    transcript_interaction_id: Some(uuid::Uuid::new_v4().to_string()),
                    expected_member: crate::runtime::bridge_protocol::BridgeMemberIncarnation {
                        mob_id: "mob".to_string(),
                        agent_identity: "member".to_string(),
                        host_id: "host".to_string(),
                        binding_generation: 1,
                        member_session_id: SessionId::new().to_string(),
                        generation: 1,
                        fence_token: 1,
                    },
                    outcome_tracking: Some(
                        crate::runtime::bridge_protocol::BridgeOutcomeTracking::Interaction,
                    ),
                }),
            )
            .await
            .expect_err("default provisioner must reject placed correlation authority");

        assert!(matches!(error, MobError::UnsupportedForMode { .. }));
        assert!(!provisioner.started.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn default_correlation_lane_delegates_only_without_placed_context() {
        let provisioner = MockProvisioner::new();
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());

        let receipt = provisioner
            .start_turn_with_correlation(&member_ref, start_turn_request(), None)
            .await
            .expect("unplaced default lane delegates to start_turn");

        assert_eq!(receipt, None);
        assert!(provisioner.started.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn commit_returns_member_ref() {
        let provisioner = Arc::new(MockProvisioner::new());
        let session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(session_id.clone());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(
            member_ref.clone(),
            member_identity,
            provisioner.clone(),
            OperationId::new(),
            ProvisionSessionOrigin::Fresh,
            None,
        );
        let committed_ref = guard.commit().unwrap();
        assert_eq!(committed_ref, member_ref);
        assert!(!provisioner.retired.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn rollback_retires_member() {
        let provisioner = Arc::new(MockProvisioner::new());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(
            member_ref,
            member_identity,
            provisioner.clone(),
            OperationId::new(),
            ProvisionSessionOrigin::Fresh,
            None,
        );
        guard.rollback().await.unwrap();
        assert!(provisioner.retired.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn resumed_rollback_restores_without_retiring_member() {
        let provisioner = Arc::new(MockProvisioner::new());
        let guard = PendingProvision::new(
            MemberRef::from_bridge_session_id(SessionId::new()),
            AgentIdentity::from("resumed-member"),
            provisioner.clone(),
            OperationId::new(),
            ProvisionSessionOrigin::ResumedDurable,
            Some(ResumedMemberRollbackAuthority::for_test()),
        );

        guard.rollback().await.unwrap();

        assert!(provisioner.restored.load(Ordering::Acquire));
        assert!(!provisioner.retired.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn rollback_failure_returns_error_without_drop_panic() {
        let provisioner = Arc::new(MockProvisioner::failing_retire());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(
            member_ref,
            member_identity,
            provisioner.clone(),
            OperationId::new(),
            ProvisionSessionOrigin::Fresh,
            None,
        );
        let error = guard
            .rollback()
            .await
            .expect_err("rollback failure should be returned to caller");

        assert!(provisioner.retired.load(Ordering::Acquire));
        assert!(error.to_string().contains("retire failed"));
    }

    #[tokio::test]
    async fn member_ref_accessor_returns_ref() {
        let provisioner = Arc::new(MockProvisioner::new());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(
            member_ref.clone(),
            member_identity,
            provisioner,
            OperationId::new(),
            ProvisionSessionOrigin::Fresh,
            None,
        );
        assert_eq!(
            guard.member_ref().expect("live provision member ref"),
            &member_ref
        );
        let _ = guard.commit(); // consume to avoid Drop panic
    }

    #[tokio::test]
    async fn agent_identity_accessor() {
        let provisioner = Arc::new(MockProvisioner::new());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(
            member_ref,
            member_identity.clone(),
            provisioner,
            OperationId::new(),
            ProvisionSessionOrigin::Fresh,
            None,
        );
        assert_eq!(guard.member_identity(), &member_identity);
        let _ = guard.commit(); // consume
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    #[should_panic(expected = "PendingProvision dropped without commit or rollback")]
    async fn drop_without_consume_panics_in_debug() {
        let provisioner = Arc::new(MockProvisioner::new());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("test-member");

        let _guard = PendingProvision::new(
            member_ref,
            member_identity,
            provisioner,
            OperationId::new(),
            ProvisionSessionOrigin::Fresh,
            None,
        );
        // dropped without commit or rollback
    }
}
