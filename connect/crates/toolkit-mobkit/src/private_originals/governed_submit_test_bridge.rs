//! Faults surround actual native delivery. Helper controls also hold before
//! native lookup; all ordinary bridge operations and results are delegated.

use super::*;
use meerkat_core::types::HandlingMode;
use meerkat_mob::ids::AgentIdentity as MobAgentIdentity;
use meerkat_mobkit::actor_loop_health::ActorLoopHealth;
use meerkat_mobkit::identity_first::*;

pub(super) struct ObservedBridge {
    pub inner: MobSessionBridge,
    pub fault: Arc<DeliveryFault>,
    pub publication: meerkat_authorization::publication::LocalAuthorizationPublication,
}

impl ObservedBridge {
    async fn observe_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
        internal: bool,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        if self.fault.mode.helper_reprepare() {
            if self.fault.helper_session.get() != Some(session_id) {
                return self.delegate_governed_delivery(runtime_id, session_id, input, custody, internal).await;
            }
            return self.observe_helper_reprepare(runtime_id, session_id, input, custody, internal).await;
        }
        if matches!(self.fault.mode, DeliveryMode::RefuseSecondBeforeLookup)
            && self.fault.accepted.load(Ordering::SeqCst) >= 1
        {
            // The real B binder and evidence write completed, but the actual
            // native bridge has not been called for B. No admission is invented.
            return Err(BridgeAdmissionError::GovernedInput(GovernedInputError::AdmissionUnresolved {
                detail: "injected refusal before second native lookup".into(),
            }));
        }
        let helper_control = matches!(self.fault.mode,
            DeliveryMode::HoldHelperBeforeLookup | DeliveryMode::HoldHelperAcceptedReply);
        if helper_control {
            if self.fault.helper_session.get() != Some(session_id) {
                return self.delegate_governed_delivery(runtime_id, session_id, input, custody, internal).await;
            }
            // The production helper binder already returned its real input and
            // fence. Preserve both, and hold just before the native bridge's
            // member lookups; the native bridge performs those lookups itself.
            {
                let mut captured = self.fault.helper_input.lock().unwrap();
                assert!(captured.replace(input.id().clone()).is_none(),
                    "one actual helper delivery at this boundary");
            }
            if matches!(self.fault.mode, DeliveryMode::HoldHelperBeforeLookup) {
                self.fault.entered.notify_one();
                self.fault.release.acquire().await.unwrap().forget();
            }
        }
        let real = self
            .delegate_governed_delivery(runtime_id, session_id, input, custody, internal)
            .await;
        if helper_control {
            self.fault.helper_returns.fetch_add(1, Ordering::SeqCst);
        }
        if matches!(&real, Err(BridgeAdmissionError::GovernedInput(GovernedInputError::Mob(error)))
            if matches!(**error, meerkat_mob::MobError::MemberInputRefused { .. }))
        {
            self.fault.refused.fetch_add(1, Ordering::SeqCst);
        }
        if !matches!(&real, Ok(GovernedDelivery { admission: Some(a), .. }) if !a.deduplicated) {
            return real;
        }
        let accepted = self.fault.accepted.fetch_add(1, Ordering::SeqCst) + 1;
        match self.fault.mode {
            DeliveryMode::Normal | DeliveryMode::HoldHelperBeforeLookup | DeliveryMode::RefuseSecondBeforeLookup
                | DeliveryMode::HelperPolicyChangesOnce | DeliveryMode::HelperPolicyChangesAlways
                | DeliveryMode::HelperPolicyChangesThenUnresolved | DeliveryMode::HelperPolicyChangesThenRetirement
                | DeliveryMode::HelperPolicyChangesThenDeadline => {}
            DeliveryMode::HoldAccepted | DeliveryMode::HoldHelperAcceptedReply => {
                // Native admission is real and its reply is retained unchanged.
                // IdentityRuntime owns the entry permit and finishes it only
                // after this bridge await returns and it observes settlement.
                self.fault.entered.notify_one();
                self.fault.release.acquire().await.unwrap().forget();
            }
            DeliveryMode::LoseAcceptedAnswer | DeliveryMode::LoseSecondAcceptedAnswer if accepted >= 2
                || matches!(self.fault.mode, DeliveryMode::LoseAcceptedAnswer) => {
                return Err(BridgeAdmissionError::GovernedInput(
                    GovernedInputError::AdmissionUnresolved {
                        detail: "injected loss after native fresh acceptance".into(),
                    },
                ));
            }
            DeliveryMode::PoisonPublicationAfterAcceptance | DeliveryMode::PoisonPublicationAfterSecondAcceptance
                if accepted >= 2 || matches!(self.fault.mode, DeliveryMode::PoisonPublicationAfterAcceptance) => {
                // Poison this real publication only after native accepted.
                // Its ordinary source-publication path must report failure;
                // this adapter never manufactures an admission or settlement.
                let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _guard = self.publication.begin_owner_change().unwrap();
                    panic!("injected shared publication failure after native acceptance");
                }));
                assert!(poisoned.is_err());
            }
            DeliveryMode::LoseAcceptedAnswer | DeliveryMode::LoseSecondAcceptedAnswer
                | DeliveryMode::PoisonPublicationAfterAcceptance | DeliveryMode::PoisonPublicationAfterSecondAcceptance => {}
        }
        real
    }

    async fn observe_helper_reprepare(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
        internal: bool,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        let ordinal = {
            let mut inputs = self.fault.helper_inputs.lock().unwrap();
            // Capture the actual bound input, without reconstructing authority.
            inputs.push(serde_json::to_value(&input).unwrap());
            inputs.len()
        };
        if ordinal == 1 {
            *self.fault.helper_input.lock().unwrap() = Some(input.id().clone());
        }
        let _change_arm = if ordinal == 1
            || matches!(self.fault.mode, DeliveryMode::HelperPolicyChangesAlways)
        {
            let association = input.header().authority_association.as_ref()
                .expect("actual bound helper association").clone();
            assert_eq!(association.candidate().target.context.as_str(), session_id.to_string());
            assert!(self.fault.helper_admission_change.lock().unwrap().replace(association).is_none());
            Some(AdmissionChangeArm(self.fault.clone()))
        } else {
            None
        };
        let real = self.delegate_governed_delivery(runtime_id, session_id, input, custody, internal).await;
        self.fault.helper_returns.fetch_add(1, Ordering::SeqCst);
        // Only closed native result fields are retained. The adapter neither
        // creates a refusal nor substitutes the desired reason for another one.
        let mut observed = serde_json::json!({
            "event": "fixture_helper_native_return", "attempt": ordinal,
            "category": "other_bridge_error",
        });
        match &real {
            Ok(delivery) => {
                observed["category"] = serde_json::json!("native_return");
                observed["admission_present"] = serde_json::json!(delivery.admission.is_some());
                if let Some(admission) = &delivery.admission {
                    observed["deduplicated"] = serde_json::json!(admission.deduplicated);
                    observed["session_matches"] = serde_json::json!(&admission.session_id == session_id);
                }
            }
            Err(BridgeAdmissionError::GovernedInput(GovernedInputError::Mob(error))) => {
                observed["category"] = serde_json::json!("mob_error");
                if let meerkat_mob::MobError::MemberInputRefused { session_id: actual, error } = error.as_ref() {
                    observed["category"] = serde_json::json!("member_input_refused");
                    observed["session_matches"] = serde_json::json!(actual == session_id);
                    match error {
                        meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable { reason } => {
                            observed["readiness"] = serde_json::json!(format!("{reason:?}"));
                        }
                        meerkat_runtime::RuntimeDriverError::InputRefused { refusal } => {
                            observed["refusal_kind"] = serde_json::json!(format!("{:?}", refusal.kind()));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        eprintln!("{observed}");
        let policy_changed = matches!(&real,
            Err(BridgeAdmissionError::GovernedInput(GovernedInputError::Mob(error)))
            if matches!(error.as_ref(), meerkat_mob::MobError::MemberInputRefused {
                session_id: refused_session,
                error: meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: meerkat_core::authorization::ControllerReadinessFailure::PolicyChanged,
                },
            } if refused_session == session_id));
        if policy_changed {
            self.fault.helper_policy_changed.fetch_add(1, Ordering::SeqCst);
            self.fault.refused.fetch_add(1, Ordering::SeqCst);
        }
        let fresh = matches!(&real, Ok(GovernedDelivery { admission: Some(a), .. }) if !a.deduplicated);
        if fresh { self.fault.accepted.fetch_add(1, Ordering::SeqCst); }
        if ordinal == 1 {
            // The test inspects the actual return before allowing re-entry. A
            // wrong return still reaches this gate and fails the owning checks.
            self.fault.entered.notify_one();
            self.fault.release.acquire().await.unwrap().forget();
        }
        if fresh && matches!(self.fault.mode, DeliveryMode::HelperPolicyChangesThenUnresolved) {
            return Err(BridgeAdmissionError::GovernedInput(GovernedInputError::AdmissionUnresolved {
                detail: "injected loss after actual helper fresh acceptance".into(),
            }));
        }
        real
    }

    async fn delegate_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
        internal: bool,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        if internal {
            self.inner
                .begin_internal_governed_delivery(runtime_id, session_id, input, custody)
                .await
        } else {
            self.inner
                .begin_governed_delivery(runtime_id, session_id, input, custody)
                .await
        }
    }
}

/// Scope the one exact helper admission fault to its actual bridge call,
/// including cancellation before the operation owner consumes it.
struct AdmissionChangeArm(Arc<DeliveryFault>);
impl Drop for AdmissionChangeArm {
    fn drop(&mut self) { self.0.helper_admission_change.lock().unwrap().take(); }
}

/// Forward the real policy unchanged, then invalidate its enclosing native
/// admission observation. The earlier source/grant lookups remain unblocked.
/// Native detects the changed stamp and produces the actual typed refusal.
pub(super) struct AdmissionChangingOwner {
    pub inner: Arc<dyn OperationPolicyOwner>,
    pub fault: Arc<DeliveryFault>,
    pub publication: meerkat_authorization::publication::LocalAuthorizationPublication,
}
impl OperationPolicyOwner for AdmissionChangingOwner {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        let allowance = self.inner.authorize_controller_admission(association, facts, now_ms)?;
        let selected = {
            let mut selected = self.fault.helper_admission_change.lock().unwrap();
            if selected.as_ref() == Some(association) { selected.take(); true } else { false }
        };
        if selected {
            // No semantic policy fact is changed. This bounded fault advances
            // the actual stamp after its genuine policy read, before Core's
            // enclosing currentness check. No lock or writer crosses an await.
            match self.publication.try_begin_owner_change() {
                Ok(change) => {
                    drop(change);
                    self.fault.publication_changes.fetch_add(1, Ordering::SeqCst);
                }
                Err(_) => { self.fault.publication_change_failures.fetch_add(1, Ordering::SeqCst); }
            }
        }
        Ok(allowance)
    }

    fn authorize_tool_application(
        &self, control: &meerkat_core::ToolApplicationControlRequest,
        binding: &PreparedAuthorizationBinding, now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.inner.authorize_tool_application(control, binding, now_ms)
    }
    fn authorize_context_control(
        &self, control: &meerkat_core::service::SystemContextControlRequest,
        binding: &PreparedAuthorizationBinding, now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.inner.authorize_context_control(control, binding, now_ms)
    }
    fn authorize_mcp_attachment(
        &self, control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding, now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.inner.authorize_mcp_attachment(control, binding, now_ms)
    }
    fn authorize_operation(
        &self, association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding, purpose: LocalPolicyPurpose, now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.inner.authorize_operation(association, binding, purpose, now_ms)
    }
    fn review_action_account(
        &self, association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding, now_ms: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        self.inner.review_action_account(association, binding, now_ms)
    }
}

#[async_trait]
impl SessionBridge for ObservedBridge {
    async fn begin_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.observe_governed_delivery(runtime_id, session_id, input, custody, false)
            .await
    }

    async fn begin_internal_governed_delivery(
        &self,
        runtime_id: &AgentRuntimeId,
        session_id: &SessionId,
        input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.observe_governed_delivery(runtime_id, session_id, input, custody, true)
            .await
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

    // Keep Core as the owner of bindings, lifecycle and durable observations.
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.inner.create_session(identity, spec, draft).await
    }

    async fn create_session_held(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
        pass: &HeldCreatePass,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.inner
            .create_session_held(identity, spec, draft, pass)
            .await
    }

    async fn reset_member_to_successor(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        self.inner
            .reset_member_to_successor(identity, spec, draft)
            .await
    }

    async fn capture_queued_member_inputs(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<QueuedMemberInputs, BridgeError> {
        self.inner
            .capture_queued_member_inputs(identity, session_id)
            .await
    }

    async fn readmit_queued_member_inputs(
        &self,
        identity: &AgentIdentity,
        session_id: &meerkat_core::types::SessionId,
        inputs: QueuedMemberInputs,
    ) {
        self.inner
            .readmit_queued_member_inputs(identity, session_id, inputs)
            .await;
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

    fn declared_role_migration(
        &self,
        identity: &AgentIdentity,
    ) -> Option<meerkat_mob::ProfileName> {
        self.inner.declared_role_migration(identity)
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

    async fn identity_intent_present(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<bool>, BridgeError> {
        self.inner.identity_intent_present(identity).await
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.inner.member_binding_history().await
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

    async fn identity_labelled_members(
        &self,
    ) -> Result<Vec<meerkat_mob::AgentIdentity>, BridgeError> {
        self.inner.identity_labelled_members().await
    }

    async fn durable_session_held(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        self.inner.durable_session_held(session_id).await
    }

    async fn session_outlives_member(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, BridgeError> {
        self.inner.session_outlives_member(session_id).await
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
}
