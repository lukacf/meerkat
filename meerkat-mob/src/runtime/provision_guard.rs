//! Linear-type spawn guard for provisioned members.
//!
//! [`PendingProvision`] wraps a provisioned [`MemberRef`] and enforces at the
//! type level that the resource is either **committed** (added to the roster)
//! or **rolled back** (session archived). Dropping without consuming is a bug
//! and always panics.
//!
//! The `#[must_use]` attribute on the type ensures the compiler warns if the
//! value is discarded.

use super::provisioner::MobProvisioner;
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
    committed: bool,
    rollback_attempted: bool,
}

impl PendingProvision {
    pub(super) fn new(
        member_ref: MemberRef,
        agent_identity: AgentIdentity,
        provisioner: Arc<dyn MobProvisioner>,
    ) -> Self {
        Self {
            member_ref: Some(member_ref),
            agent_identity,
            provisioner,
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

    /// Roll back the provision, archiving the session.
    pub(super) async fn rollback(mut self) -> Result<(), MobError> {
        let member_ref = self.take_member_ref("rollback")?;
        match self.provisioner.retire_member(&member_ref).await {
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
    pub(super) fn member_ref(&self) -> &MemberRef {
        // `member_ref` is `Some` from construction until `commit()` or
        // `rollback()` consume `self`. Borrowing through `&self` cannot happen
        // after consumption.
        self.member_ref
            .as_ref()
            .unwrap_or_else(|| unreachable!("PendingProvision::member_ref consumed before access"))
    }

    /// The meerkat ID associated with this provision.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn member_identity(&self) -> &AgentIdentity {
        &self.agent_identity
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
        fail_retire: bool,
    }

    impl MockProvisioner {
        fn new() -> Self {
            Self {
                retired: AtomicBool::new(false),
                fail_retire: false,
            }
        }

        fn failing_retire() -> Self {
            Self {
                retired: AtomicBool::new(false),
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

        async fn retire_member(&self, _member_ref: &MemberRef) -> Result<(), MobError> {
            self.retired.store(true, Ordering::Release);
            if self.fail_retire {
                return Err(MobError::Internal("retire failed".to_string()));
            }
            Ok(())
        }

        async fn interrupt_member(&self, _member_ref: &MemberRef) -> Result<(), MobError> {
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

    #[tokio::test]
    async fn commit_returns_member_ref() {
        let provisioner = Arc::new(MockProvisioner::new());
        let session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(session_id.clone());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(member_ref.clone(), member_identity, provisioner.clone());
        let committed_ref = guard.commit().unwrap();
        assert_eq!(committed_ref, member_ref);
        assert!(!provisioner.retired.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn rollback_retires_member() {
        let provisioner = Arc::new(MockProvisioner::new());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(member_ref, member_identity, provisioner.clone());
        guard.rollback().await.unwrap();
        assert!(provisioner.retired.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn rollback_failure_returns_error_without_drop_panic() {
        let provisioner = Arc::new(MockProvisioner::failing_retire());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(member_ref, member_identity, provisioner.clone());
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

        let guard = PendingProvision::new(member_ref.clone(), member_identity, provisioner);
        assert_eq!(guard.member_ref(), &member_ref);
        let _ = guard.commit(); // consume to avoid Drop panic
    }

    #[tokio::test]
    async fn agent_identity_accessor() {
        let provisioner = Arc::new(MockProvisioner::new());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("test-member");

        let guard = PendingProvision::new(member_ref, member_identity.clone(), provisioner);
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

        let _guard = PendingProvision::new(member_ref, member_identity, provisioner);
        // dropped without commit or rollback
    }
}
