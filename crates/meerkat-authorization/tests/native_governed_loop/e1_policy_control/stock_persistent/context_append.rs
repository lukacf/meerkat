//! B7 positive and revoked controls through the stock service/native owners.
//! The fixture owns only its application mandate and source/destination ACLs.
use super::*;
use meerkat_core::authorization::{
    OwnerQualifiedTarget, PublicationRecipient, SourceAuthorizationTarget, SourceAuthorizationUse,
};
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextStatus, ContextControlFacts,
    SessionControlError, SessionServiceControlExt, SystemContextControlRequest,
};
use meerkat_core::session::context_control::{
    ContextControlAuditOutcome, ContextControlAuditRecord, ContextControlPolicyObservation,
};

#[derive(Clone, Copy)]
pub(super) enum Permission {
    Invoke = 0,
    Source = 1,
    Destination = 2,
}

/// Actual configured application owner for this fixture. Its accepted changes
/// use the same publication as the native grant authority.
pub(super) struct ControlOwner {
    http: Arc<HttpRecordOwner>,
    publication: LocalAuthorizationPublication,
    allowed: Mutex<[bool; 3]>,
}

struct ControlReceipt {
    owner: Arc<ControlOwner>,
    session_id: SessionId,
    request_digest: String,
}

fn destination(session_id: &SessionId) -> OwnerQualifiedTarget {
    OwnerQualifiedTarget {
        authority: principal("control-destination-owner"),
        namespace: Arc::from("session-context"),
        id: Arc::from(session_id.to_string()),
    }
}

fn source() -> SourceAuthorizationTarget {
    SourceAuthorizationTarget::External(source_target())
}

fn source_target() -> OwnerQualifiedTarget {
    OwnerQualifiedTarget {
        authority: principal("control-source-owner"),
        namespace: Arc::from("fixture-source"),
        id: Arc::from("record-7"),
    }
}

fn same_target(left: &OwnerQualifiedTarget, right: &OwnerQualifiedTarget) -> bool {
    left.authority == right.authority && left.namespace == right.namespace && left.id == right.id
}

fn is_source(target: &SourceAuthorizationTarget) -> bool {
    matches!(target, SourceAuthorizationTarget::External(actual) if same_target(actual, &source_target()))
}

impl ControlOwner {
    pub(super) fn new(
        http: Arc<HttpRecordOwner>,
        publication: LocalAuthorizationPublication,
    ) -> Arc<Self> {
        Arc::new(Self {
            http,
            publication,
            allowed: Mutex::new([true; 3]),
        })
    }

    fn set(&self, permission: Permission, allowed: bool) {
        let _publication = self.publication.begin_owner_change().unwrap();
        self.allowed.lock().unwrap()[permission as usize] = allowed;
    }

    fn control(
        self: &Arc<Self>,
        session_id: &SessionId,
        request: AppendSystemContextRequest,
    ) -> Arc<SystemContextControlRequest> {
        let receipt = Arc::new(ControlReceipt {
            owner: self.clone(),
            session_id: session_id.clone(),
            request_digest: ContextControlAuditRecord::digest_request(&request).unwrap(),
        });
        SystemContextControlRequest::from_trusted_ingress(
            session_id.clone(),
            request,
            ContextControlFacts {
                requester: principal("control-requester"),
                actor: principal("control-executor"),
                realm: RealmId::parse("native-loop").unwrap(),
                source: source(),
                destination: destination(session_id),
            },
            receipt,
        )
        .unwrap()
    }

    fn require(
        &self,
        control: &SystemContextControlRequest,
        binding: &PreparedAuthorizationBinding,
        permission: Permission,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        let receipt = control.evidence::<ControlReceipt>().ok_or_else(denied)?;
        let facts = control.facts();
        if !std::ptr::eq(receipt.owner.as_ref(), self)
            || &receipt.session_id != control.session_id()
            || receipt.request_digest
                != ContextControlAuditRecord::digest_request(control.request())?
            || facts.requester != principal("control-requester")
            || facts.actor != principal("control-executor")
            || facts.realm != RealmId::parse("native-loop").unwrap()
            || !is_source(&facts.source)
            || !same_target(&facts.destination, &destination(control.session_id()))
            || &binding.facts().operation_id != control.operation_id()
            || !self.allowed.lock().unwrap()[permission as usize]
        {
            return Err(denied().into());
        }
        Ok(())
    }
}

impl AdmittedWorkPolicyOwner for ControlOwner {
    fn authorize_context_control(
        &self,
        control: &SystemContextControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        self.require(control, binding, Permission::Invoke)?;
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }

    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        InvocationOwner.authorize_admitted_work(association, binding, purpose, now_ms)
    }
}

impl OperationPolicyOwner for ControlOwner {
    fn authorize_context_control(
        &self,
        control: &SystemContextControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let (permission, action_name) = match &binding.facts().operation {
            AuthorizationOperation::Source(facts)
                if is_source(&facts.target) && facts.usage == SourceAuthorizationUse::Hydrate =>
            {
                (Permission::Source, "read")
            }
            AuthorizationOperation::Publication(facts)
                if matches!(&facts.recipient, PublicationRecipient::Destination(target)
                    if same_target(target, &destination(control.session_id()))) =>
            {
                (Permission::Destination, "publish")
            }
            _ => return Err(denied().into()),
        };
        self.require(control, binding, permission)?;
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: action(action_name),
                resource_domain: domain(),
                processor: ProcessorRef::Principal {
                    principal: principal("control-executor"),
                },
                audience: AudienceRef::Principal {
                    principal: principal("control-requester"),
                },
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }

    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        self.http
            .authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.http
            .authorize_operation(association, binding, purpose, now_ms)
    }
}

pub(super) const PERMITTED_CONTEXT: &str = "b7-current-source-and-destination-permitted";
pub(super) const REFUSED_CONTEXT: &str = "b7-revoked-context-must-never-enter";

pub(super) async fn exercise(
    owner: &Arc<ControlOwner>,
    service: &Arc<meerkat::PersistentSessionService<FactoryAgentBuilder>>,
    machine: &Arc<MeerkatMachine>,
    store: &Arc<dyn RuntimeStore>,
    session_id: &SessionId,
) {
    let runtime = LogicalRuntimeId::for_session(session_id);
    let before = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    let before_messages = serde_json::to_value(before.session().messages()).unwrap();
    let before_inputs =
        serde_json::to_value(store.load_input_states_strict(&runtime).await.unwrap()).unwrap();

    // All three owners can independently veto. Prepare while allowed, poll
    // the real service while the actual actor is held, then revoke before
    // releasing it. This is final entry revalidation, not a preparation-only test.
    for permission in [
        Permission::Invoke,
        Permission::Source,
        Permission::Destination,
    ] {
        let control = owner.control(
            session_id,
            AppendSystemContextRequest::from_text(REFUSED_CONTEXT),
        );
        let preparation = machine.prepare_context_append_observed(control.clone());
        let prepared_policy = preparation.policy.expect("coherent native policy read");
        let prepared = preparation.result.unwrap();
        let lease = service
            .acquire_live_session_actor_turn_boundary_lease(session_id)
            .await
            .unwrap();
        let pending = service.append_authorized_system_context(prepared);
        tokio::pin!(pending);
        std::future::poll_fn(|cx| {
            use std::future::Future;
            assert!(
                pending.as_mut().poll(cx).is_pending(),
                "actual actor custody holds the append"
            );
            std::task::Poll::Ready(())
        })
        .await;
        owner.set(permission, false);
        drop(lease);
        assert!(matches!(pending.await,
            Err(SessionControlError::Authorization(meerkat_core::OperationAuthorizationError::Refused(reason)))
                if reason.kind() == OperationRefusalKind::Denied));
        let saved = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(saved.session().messages()).unwrap(),
            before_messages
        );
        let observed = saved
            .session()
            .context_control_observation(control.operation_id())
            .unwrap()
            .unwrap();
        assert_eq!(
            observed.outcome,
            ContextControlAuditOutcome::Refused {
                reason: OperationRefusalKind::Denied
            }
        );
        assert_eq!(
            observed.requester.as_ref(),
            Some(&principal("control-requester"))
        );
        assert_eq!(
            observed.actor.as_ref(),
            Some(&principal("control-executor"))
        );
        let [ContextControlPolicyObservation::LocalPublication { instance, sequence }] =
            observed.policy.as_slice()
        else {
            panic!("revoked control must retain its actual policy observation");
        };
        let ContextControlPolicyObservation::LocalPublication {
            instance: prepared_instance,
            sequence: prepared_sequence,
        } = prepared_policy;
        assert_eq!(*instance, prepared_instance);
        assert!(
            *sequence > prepared_sequence,
            "queued refusal must record the rechecked publication, not the earlier allow"
        );
        // The full owner API must also retain a denial discovered at preparation.
        let denied_at_prepare = owner.control(
            session_id,
            AppendSystemContextRequest::from_text(REFUSED_CONTEXT),
        );
        assert!(
            matches!(service.append_authenticated_system_context(denied_at_prepare.clone()).await,
            Err(SessionControlError::Authorization(meerkat_core::OperationAuthorizationError::Refused(reason)))
                if reason.kind() == OperationRefusalKind::Denied)
        );
        let saved = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        let denied_observation = saved
            .session()
            .context_control_observation(denied_at_prepare.operation_id())
            .unwrap()
            .unwrap();
        assert_eq!(
            denied_observation.outcome,
            ContextControlAuditOutcome::Refused {
                reason: OperationRefusalKind::Denied
            }
        );
        assert_eq!(
            denied_observation.policy, observed.policy,
            "initial denial and queued recheck read the same unchanged owner publication"
        );
        owner.set(permission, true);
    }

    let mut request = AppendSystemContextRequest::from_text(PERMITTED_CONTEXT);
    request.idempotency_key = Some("b7-authorized-control-once".into());
    let mut appended_policy = None;
    for expected in [
        AppendSystemContextStatus::Applied,
        AppendSystemContextStatus::Duplicate,
    ] {
        let control = owner.control(session_id, request.clone());
        let result = service
            .append_authenticated_system_context(control.clone())
            .await
            .unwrap();
        assert_eq!(result.status, expected);
        let saved = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        let observed = saved
            .session()
            .context_control_observation(control.operation_id())
            .unwrap()
            .unwrap();
        assert!(matches!(
            observed.policy.as_slice(),
            [ContextControlPolicyObservation::LocalPublication { .. }]
        ));
        if let Some(policy) = &appended_policy {
            assert_eq!(
                &observed.policy, policy,
                "duplicate checks retain the unchanged publication, not invented revisions"
            );
        } else {
            appended_policy = Some(observed.policy.clone());
        }
        match expected {
            AppendSystemContextStatus::Applied => assert!(matches!(
                observed.outcome,
                ContextControlAuditOutcome::Appended { .. }
            )),
            AppendSystemContextStatus::Duplicate => {
                assert_eq!(observed.outcome, ContextControlAuditOutcome::Duplicate);
            }
        }
    }
    let saved = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.session().messages().len(),
        before.session().messages().len() + 1
    );
    assert!(
        !serde_json::to_string(saved.session().messages())
            .unwrap()
            .contains(REFUSED_CONTEXT)
    );
    assert_eq!(
        serde_json::to_value(store.load_input_states_strict(&runtime).await.unwrap()).unwrap(),
        before_inputs,
        "control observations and permitted System context never manufacture a native input"
    );
}
