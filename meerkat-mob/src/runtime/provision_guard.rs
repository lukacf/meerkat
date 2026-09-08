//! Linear-type spawn guard for provisioned members.
//!
//! [`PendingProvision`] wraps a provisioned [`MemberRef`] and enforces at the
//! type level that the resource is either **committed** (added to the roster)
//! or **rolled back** (session archived). Dropping without consuming is a bug
//! and always panics.
//!
//! The `#[must_use]` attribute on the type ensures the compiler warns if the
//! value is discarded.
//!
//! A failed rollback is not a settled outcome. [`PendingProvision::rollback`]
//! reports the failure but destroys the guard, so the exact owner-issued
//! cleanup authority (member ref, operation id, origin, and the resumed
//! attachment rollback witness) is lost with it.
//! [`PendingProvision::rollback_retaining_custody`] keeps that authority in a
//! [`RetainedProvisionCustody`] so a cancelling caller can stay in Cancelling
//! and retry the exact owner cleanup instead of terminalizing on an
//! unproven compensation.

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
    /// Set when a caller explicitly took over or abandoned the retained
    /// cleanup custody after a failed rollback, so `Drop` does not emit a
    /// second, less informative leak record for the same resource.
    custody_settled: bool,
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
            custody_settled: false,
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
    ///
    /// A failed rollback consumes the guard: the resource is retained but its
    /// exact cleanup authority is not returned. Prefer
    /// [`Self::rollback_retaining_custody`] on any path where the caller can
    /// keep cancelling and retry.
    pub(super) async fn rollback(mut self) -> Result<(), MobError> {
        self.attempt_rollback().await
    }

    /// Roll back the provision, preserving exact cleanup custody on failure.
    ///
    /// On `Err` the returned [`RetainedProvisionCustody`] still owns this
    /// provision's member ref, operation id, session origin, and resumed
    /// attachment rollback authority, so the caller can retry the exact
    /// owner-issued compensation. Nothing is retired, archived, or forgotten
    /// by the failure itself.
    pub(super) async fn rollback_retaining_custody(
        mut self,
    ) -> Result<(), RetainedProvisionCustody> {
        match self.attempt_rollback().await {
            Ok(()) => Ok(()),
            Err(error) => Err(RetainedProvisionCustody {
                provision: self,
                error,
            }),
        }
    }

    /// Shared rollback mechanics. On failure the member ref is restored so the
    /// guard remains a usable retry handle for the exact same resource.
    async fn attempt_rollback(&mut self) -> Result<(), MobError> {
        let member_ref = self.take_member_ref("rollback")?;
        let rollback = match self.session_origin {
            ProvisionSessionOrigin::Fresh => self
                .provisioner
                .retire_member(&member_ref)
                .await
                .map(|_| ()),
            ProvisionSessionOrigin::ResumedDurable | ProvisionSessionOrigin::RevivedRetired => {
                match self.rollback_authority.as_ref() {
                    Some(rollback_authority) => {
                        self.provisioner
                            .restore_resumed_member(
                                &member_ref,
                                &self.operation_id,
                                self.session_origin,
                                rollback_authority,
                            )
                            .await
                    }
                    None => Err(MobError::Internal(format!(
                        "resumed provision '{}' lost its exact rollback attachment authority",
                        self.agent_identity
                    ))),
                }
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
        if self.custody_settled {
            return;
        }
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

/// Exact, retryable cleanup custody for a provision whose rollback failed.
///
/// This is the settlement carrier for a cancelling caller: the compensation
/// did NOT complete, the member resource is still retained, and the exact
/// authority needed to retry that compensation is still owned here. Callers
/// must keep their own cancellation pending until [`Self::retry`] reports
/// `Ok`, or until they explicitly [`Self::abandon`] the custody and surface
/// the uncertainty. Dropping it without either records a leak.
#[must_use = "retained cleanup custody must be retried or explicitly abandoned"]
pub(super) struct RetainedProvisionCustody {
    provision: PendingProvision,
    error: MobError,
}

impl RetainedProvisionCustody {
    /// The failure that left this custody retained.
    pub(super) fn error(&self) -> &MobError {
        &self.error
    }

    /// The still-retained member resource, when the guard held one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn member_ref(&self) -> Result<&MemberRef, MobError> {
        self.provision.member_ref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn member_identity(&self) -> &AgentIdentity {
        self.provision.member_identity()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn session_origin(&self) -> ProvisionSessionOrigin {
        self.provision.session_origin()
    }

    /// Retry the exact owner-issued cleanup. On failure the custody is
    /// returned again with the newest error, so retry loops never lose it.
    pub(super) async fn retry(self) -> Result<(), Self> {
        self.provision.rollback_retaining_custody().await
    }

    /// Give up on the retained cleanup explicitly, recording the reason once
    /// and returning the last failure for the caller to surface. The resource
    /// stays retained on the host; this is an admission of uncertainty, never
    /// a claim that compensation completed.
    pub(super) fn abandon(mut self, reason: &str) -> MobError {
        tracing::error!(
            agent_identity = %self.provision.agent_identity,
            member_ref = ?self.provision.member_ref,
            session_origin = ?self.provision.session_origin,
            error = %self.error,
            reason,
            "retained provision cleanup custody abandoned without proven compensation"
        );
        self.provision.custody_settled = true;
        self.error
    }

    /// Split the custody into its retryable guard and the last failure. The
    /// caller becomes responsible for consuming the returned guard.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn into_parts(self) -> (PendingProvision, MobError) {
        (self.provision, self.error)
    }
}

impl std::fmt::Debug for RetainedProvisionCustody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RetainedProvisionCustody")
            .field("agent_identity", &self.provision.agent_identity)
            .field("session_origin", &self.provision.session_origin)
            .field("error", &self.error.to_string())
            .finish()
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Notify;

    struct MockProvisioner {
        retired: AtomicBool,
        restored: AtomicBool,
        started: AtomicBool,
        fail_retire: bool,
        /// Number of remaining rollback attempts that must fail before the
        /// exact cleanup is allowed to succeed.
        rollback_failures_remaining: AtomicUsize,
        /// A successor attachment took over the durable session. An exact
        /// rollback must refuse and must not retire it.
        successor_attachment: AtomicBool,
        successor_removed: AtomicBool,
        restore_attempts: AtomicUsize,
        /// Fail `provision_member` so the default settled lane can be observed.
        fail_provision: bool,
        /// Signalled when the owner-side cleanup work has started.
        cleanup_entered: Arc<Notify>,
        /// Awaited by the owner-side cleanup work before it settles.
        cleanup_release: Arc<Notify>,
        barrier_cleanup: bool,
    }

    impl MockProvisioner {
        fn new() -> Self {
            Self {
                retired: AtomicBool::new(false),
                restored: AtomicBool::new(false),
                started: AtomicBool::new(false),
                fail_retire: false,
                rollback_failures_remaining: AtomicUsize::new(0),
                successor_attachment: AtomicBool::new(false),
                successor_removed: AtomicBool::new(false),
                restore_attempts: AtomicUsize::new(0),
                fail_provision: false,
                cleanup_entered: Arc::new(Notify::new()),
                cleanup_release: Arc::new(Notify::new()),
                barrier_cleanup: false,
            }
        }

        fn failing_retire() -> Self {
            Self {
                fail_retire: true,
                ..Self::new()
            }
        }

        /// Fail the next `failures` rollback attempts, then succeed.
        fn failing_rollback_times(failures: usize) -> Self {
            Self {
                rollback_failures_remaining: AtomicUsize::new(failures),
                ..Self::new()
            }
        }

        /// Model a successor attachment that already replaced this provision's
        /// exact attachment, mirroring the witness-exact refusal in
        /// `SessionBackend::restore_failed_resume_before_receipt`.
        fn with_successor_attachment() -> Self {
            Self {
                successor_attachment: AtomicBool::new(true),
                ..Self::new()
            }
        }

        fn barriered_cleanup() -> Self {
            Self {
                barrier_cleanup: true,
                ..Self::new()
            }
        }

        fn failing_provision() -> Self {
            Self {
                fail_provision: true,
                ..Self::new()
            }
        }

        /// Consume one scheduled failure, if any remain.
        fn take_scheduled_failure(&self) -> bool {
            self.rollback_failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
        }

        async fn enter_barriered_cleanup(&self) {
            if !self.barrier_cleanup {
                return;
            }
            self.cleanup_entered.notify_one();
            self.cleanup_release.notified().await;
        }
    }

    #[async_trait]
    impl MobProvisioner for MockProvisioner {
        async fn provision_member(
            &self,
            _req: ProvisionMemberRequest,
        ) -> Result<MemberSpawnReceipt, MobError> {
            if self.fail_provision {
                return Err(MobError::Internal("provision failed".to_string()));
            }
            Ok(MemberSpawnReceipt {
                member_ref: MemberRef::from_bridge_session_id(SessionId::new()),
                direct_member_fence: None,
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
            self.restore_attempts.fetch_add(1, Ordering::AcqRel);
            self.enter_barriered_cleanup().await;
            if self.successor_attachment.load(Ordering::Acquire) {
                // Mirrors the witness-exact refusal in
                // `restore_failed_resume_before_receipt`: a successor owns the
                // durable session, so this attempt's authority compensates
                // nothing and must not touch it.
                return Err(MobError::Internal(
                    "resume rollback lost its exact runtime attachment or sidecar before durable convergence"
                        .to_string(),
                ));
            }
            if self.take_scheduled_failure() {
                return Err(MobError::Internal("restore failed".to_string()));
            }
            self.restored.store(true, Ordering::Release);
            Ok(())
        }

        async fn retire_member(
            &self,
            _member_ref: &MemberRef,
        ) -> Result<crate::machines::mob_machine::MemberSessionDisposal, MobError> {
            self.retired.store(true, Ordering::Release);
            self.enter_barriered_cleanup().await;
            if self.successor_attachment.load(Ordering::Acquire) {
                self.successor_removed.store(true, Ordering::Release);
            }
            if self.fail_retire || self.take_scheduled_failure() {
                return Err(MobError::Internal("retire failed".to_string()));
            }
            Ok(crate::machines::mob_machine::MemberSessionDisposal::Archived)
        }

        async fn retire_member_until(
            &self,
            member_ref: &MemberRef,
            _member_identity: &AgentIdentity,
            _deadline: meerkat_core::time_compat::Instant,
        ) -> Result<crate::machines::mob_machine::MemberSessionDisposal, MobError> {
            self.retire_member(member_ref).await
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

        async fn admit_tracked_turn(
            &self,
            member_ref: &MemberRef,
            _req: StartTurnRequest,
            completion_tx: crate::runtime::handle::ExactTurnCompletionSender,
            llm_identity_applied_tx: Option<
                crate::runtime::handle::MemberTurnLlmIdentityAppliedSender,
            >,
        ) -> Result<(), MobError> {
            if let Some(llm_identity_applied_tx) = llm_identity_applied_tx {
                let _ = llm_identity_applied_tx.send(Ok(None));
            }
            let session_id = member_ref
                .bridge_session_id()
                .cloned()
                .unwrap_or_else(SessionId::new);
            let _ = completion_tx.send(Ok(crate::runtime::handle::ExactTurnCompletion {
                session_id,
                terminal: crate::runtime::handle::ExactTurnTerminal::Runtime(
                    meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult,
                ),
            }));
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

        async fn prepare_member_session_for_explicit_resume(
            &self,
            _session_id: &SessionId,
            _deadline: meerkat_core::time_compat::Instant,
            _admission: Option<crate::runtime::state::LifecycleAdmissionSignal>,
        ) -> Result<bool, MobError> {
            Ok(false)
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
                    bounded_result_spec: None,
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

    #[tokio::test]
    async fn failed_rollback_retains_exact_retryable_custody() {
        let provisioner = Arc::new(MockProvisioner::failing_rollback_times(1));
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let member_identity = AgentIdentity::from("retained-member");

        let guard = PendingProvision::new(
            member_ref.clone(),
            member_identity.clone(),
            provisioner.clone(),
            OperationId::new(),
            ProvisionSessionOrigin::Fresh,
            None,
        );

        let custody = guard
            .rollback_retaining_custody()
            .await
            .expect_err("a failed rollback must return retryable custody");
        assert_eq!(custody.member_identity(), &member_identity);
        assert_eq!(
            custody
                .member_ref()
                .expect("retained custody keeps the exact member resource"),
            &member_ref
        );
        assert_eq!(custody.session_origin(), ProvisionSessionOrigin::Fresh);
        assert!(custody.error().to_string().contains("retire failed"));

        custody
            .retry()
            .await
            .map_err(|custody| custody.abandon("test retry must settle"))
            .expect("retry against the retained custody must settle the cleanup");
        assert!(provisioner.retired.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn resumed_rollback_failure_retains_authority_without_archiving_durable_continuity() {
        let provisioner = Arc::new(MockProvisioner::failing_rollback_times(1));
        let guard = PendingProvision::new(
            MemberRef::from_bridge_session_id(SessionId::new()),
            AgentIdentity::from("resumed-member"),
            provisioner.clone(),
            OperationId::new(),
            ProvisionSessionOrigin::ResumedDurable,
            Some(ResumedMemberRollbackAuthority::for_test()),
        );

        let custody = guard
            .rollback_retaining_custody()
            .await
            .expect_err("failed resume rollback must retain its exact authority");
        assert_eq!(
            custody.session_origin(),
            ProvisionSessionOrigin::ResumedDurable
        );
        assert!(!provisioner.restored.load(Ordering::Acquire));

        custody
            .retry()
            .await
            .map_err(|custody| custody.abandon("test retry must settle"))
            .expect("retained resume custody must be retryable");

        assert_eq!(provisioner.restore_attempts.load(Ordering::Acquire), 2);
        assert!(provisioner.restored.load(Ordering::Acquire));
        assert!(
            !provisioner.retired.load(Ordering::Acquire),
            "resume undo must never archive durable continuity"
        );
    }

    #[tokio::test]
    async fn exact_resumed_rollback_refuses_successor_attachment() {
        let provisioner = Arc::new(MockProvisioner::with_successor_attachment());
        let guard = PendingProvision::new(
            MemberRef::from_bridge_session_id(SessionId::new()),
            AgentIdentity::from("superseded-member"),
            provisioner.clone(),
            OperationId::new(),
            ProvisionSessionOrigin::ResumedDurable,
            Some(ResumedMemberRollbackAuthority::for_test()),
        );

        let custody = guard
            .rollback_retaining_custody()
            .await
            .expect_err("a superseded attachment must not be reported as compensated");

        assert!(
            !provisioner.successor_removed.load(Ordering::Acquire),
            "exact rollback must never remove a successor attachment"
        );
        assert!(!provisioner.restored.load(Ordering::Acquire));
        assert_eq!(provisioner.restore_attempts.load(Ordering::Acquire), 1);

        let error = custody.abandon("successor owns the durable session");
        assert!(
            error
                .to_string()
                .contains("lost its exact runtime attachment")
        );
    }

    #[tokio::test]
    async fn missing_resume_rollback_authority_retains_custody_instead_of_leaking() {
        let provisioner = Arc::new(MockProvisioner::new());
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());
        let guard = PendingProvision::new(
            member_ref.clone(),
            AgentIdentity::from("authority-less-member"),
            provisioner.clone(),
            OperationId::new(),
            ProvisionSessionOrigin::ResumedDurable,
            None,
        );

        let custody = guard
            .rollback_retaining_custody()
            .await
            .expect_err("a provision without rollback authority cannot settle");
        assert_eq!(
            custody
                .member_ref()
                .expect("the member resource stays retained"),
            &member_ref
        );
        assert!(
            custody
                .error()
                .to_string()
                .contains("lost its exact rollback attachment authority")
        );
        assert_eq!(provisioner.restore_attempts.load(Ordering::Acquire), 0);
        let _ = custody.abandon("no exact authority to retry with");
    }

    #[tokio::test]
    async fn owner_task_cleanup_completes_after_observer_handle_drop() {
        let provisioner = Arc::new(MockProvisioner::barriered_cleanup());
        let entered = Arc::clone(&provisioner.cleanup_entered);
        let release = Arc::clone(&provisioner.cleanup_release);
        let settled = Arc::new(AtomicBool::new(false));

        let owner = {
            let provisioner = Arc::clone(&provisioner);
            let settled = Arc::clone(&settled);
            tokio::spawn(async move {
                let guard = PendingProvision::new(
                    MemberRef::from_bridge_session_id(SessionId::new()),
                    AgentIdentity::from("owner-task-member"),
                    provisioner,
                    OperationId::new(),
                    ProvisionSessionOrigin::Fresh,
                    None,
                );
                match guard.rollback_retaining_custody().await {
                    Ok(()) => settled.store(true, Ordering::Release),
                    Err(custody) => {
                        let _ = custody.abandon("barriered cleanup must settle");
                    }
                }
            })
        };

        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .expect("owner cleanup must start");
        // Drop the observer handle: the owner task keeps the cleanup work.
        drop(owner);
        assert!(!settled.load(Ordering::Acquire));

        release.notify_one();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !settled.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "owner-task cleanup must settle after the observer is dropped"
            );
            tokio::task::yield_now().await;
        }
        assert!(provisioner.retired.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn default_settled_lane_reports_unproven_for_custom_provisioners() {
        use crate::runtime::provisioner::{ProvisionEffectSettlement, RuntimeRevivalIntent};

        let provisioner = MockProvisioner::failing_provision();
        let request = ProvisionMemberRequest {
            create_session: meerkat_core::service::CreateSessionRequest {
                injected_context: Vec::new(),
                model: "mock-model".to_string(),
                prompt: meerkat_core::types::ContentInput::Text("work".to_string()),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            },
            authorized_resume: None,
            session_origin: ProvisionSessionOrigin::Fresh,
            binding: crate::RuntimeBinding::Session,
            peer_name: "custom-member".to_string(),
            owner_bridge_session_id: None,
            ops_registry: None,
            generated_self_owned_operation_owner: None,
            runtime_revival_intent: RuntimeRevivalIntent::None,
            direct_member_incarnation: None,
        };

        let failure = provisioner
            .provision_member_settled(request)
            .await
            .expect_err("the failing mock must not report a receipt");

        assert_eq!(
            failure.settlement(),
            ProvisionEffectSettlement::Unproven,
            "a provisioner that owns no local materialization seam must not claim settlement"
        );
        assert!(
            failure.requires_further_cleanup(),
            "Unproven must keep compensation open, exactly like RetainedUncertain"
        );
        assert!(failure.retained_effects().is_none());
        assert!(
            failure
                .into_error()
                .to_string()
                .contains("provision failed")
        );
    }

    #[tokio::test]
    async fn default_retained_cleanup_seam_refuses_and_hands_custody_back() {
        use crate::runtime::provisioner::{
            ProvisionSettlementLedger, RetainedOperationAnchor, RetainedProvisionEffects,
        };

        fn custody(session_id: &SessionId, operation_id: &OperationId) -> RetainedProvisionEffects {
            let ledger = ProvisionSettlementLedger::default();
            ledger.set_attempt_origin(ProvisionSessionOrigin::ResumedDurable);
            ledger.set_attempt_operation_id(operation_id.clone());
            ledger.record_retained_attachment(
                session_id,
                ResumedMemberRollbackAuthority::for_test(),
                "exact attachment retirement uncertain",
            );
            ledger
                .settle_failure(MobError::Internal("provision failed".to_string()))
                .into_parts()
                .2
                .expect("a recorded witness must yield custody")
        }

        let provisioner = MockProvisioner::new();
        let session_id = SessionId::new();
        let operation_id = OperationId::new();

        let failure = provisioner
            .retry_retained_provision_cleanup(custody(&session_id, &operation_id))
            .await
            .expect_err("a provisioner that did not issue the custody must refuse");

        assert!(
            failure.is_unsupported(),
            "the default seam refuses explicitly instead of reporting a fabricated settlement"
        );
        let (retained, error) = failure.into_parts();
        assert_eq!(retained.member_ref().bridge_session_id(), Some(&session_id));
        assert_eq!(
            retained.operation_anchor(),
            &RetainedOperationAnchor::Minted(operation_id),
            "custody must come back unchanged, including its exact ops anchor"
        );
        assert!(
            matches!(error, MobError::LifecycleOperationPending { .. }),
            "an owner that cannot compensate reports pending lifecycle work, not a fabricated \
             runtime-mode refusal: {error}"
        );
        assert!(!provisioner.retired.load(Ordering::Acquire));
        assert!(!provisioner.restored.load(Ordering::Acquire));
    }
}
