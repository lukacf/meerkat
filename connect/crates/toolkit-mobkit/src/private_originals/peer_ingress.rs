//! Core-authenticated deliveries select only their receiving Add's mandate.
//! Sender facts name a reply destination, never authority or a contributor.
use super::native_operations::SelectedMember;
use super::*;
use crate::installed_host::application_operations;
use meerkat_authorization_contracts::{
    constraints::{ActionRef, DelegationDepth, DomainRestriction, ExactRestriction},
    work_association::{
        GrantTarget, InputAuthorityAssociation, TargetGrantLineage, WorkAuthorityBasis,
    },
};
use meerkat_core::{MemoryOperation, PeerSendKind, ToolOperationTarget};
use meerkat_mobkit::identity_first::{GovernedBindContext, GovernedInputBinder};
use meerkat_runtime::{
    Input, LogicalRuntimeId,
    identifiers::IdempotencyKey,
    input_authority::{
        PeerIngressBindFuture, PeerIngressBindRefusal, PeerIngressBindRequest, PeerIngressBinder,
        PeerIngressBinding, PeerIngressClaimKind, PeerIngressOrigin,
    },
};

/// Full immutable delivery coordinate in the existing source row. The digest
/// commits Core's projected input with only its observation time normalized.
/// The exact first bound input remains separately retained by source custody.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PeerSource {
    session: meerkat_core::SessionId,
    runtime: String,
    event_key: String,
    work: EvidenceId,
    origin: PeerOrigin,
    raw_sha256: String,
    raw_key: Option<IdempotencyKey>,
    input_id: meerkat_core::lifecycle::InputId,
    selection: PeerSelection,
}
/// The origin discriminator is semantic and typed. Complete native facts are
/// retained as inert evidence and compared in full on every repeated offer.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PeerOrigin {
    Comms {
        sender: meerkat_core::comms::PeerId,
        details: serde_json::Value,
    },
    Supervisor {
        details: serde_json::Value,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerSelection {
    receiver: SelectedMember,
    identity: meerkat_mobkit::identity_first::AgentIdentity,
    /// None is supervisor relay or a refused unknown comms sender, never a
    /// grant to address the supervisor or an arbitrary peer.
    sender: Option<(meerkat_core::comms::PeerId, SelectedMember)>,
}
fn unavailable() -> PeerIngressBindRefusal {
    PeerIngressBindRefusal::Unavailable("Toolkit receiver source is unavailable".into())
}
fn readiness() -> PeerIngressBindRefusal {
    PeerIngressBindRefusal::NotReady(
        meerkat_core::authorization::ControllerReadinessFailure::ExecutorUnavailable,
    )
}
fn host_error(
    error: meerkat_toolkit::host_lifetime::control::HostLifetimeError,
) -> PrivateOriginalError {
    match error {
        meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable
        | meerkat_toolkit::host_lifetime::control::HostLifetimeError::NotSettled => {
            PrivateOriginalError::Unavailable
        }
        _ => PrivateOriginalError::Denied,
    }
}
fn raw_digest(
    input: &Input,
    raw_key: &Option<IdempotencyKey>,
) -> Result<String, PrivateOriginalError> {
    let Input::Peer(mut peer) = input.clone() else {
        return Err(PrivateOriginalError::Conflict);
    };
    peer.header.timestamp = std::time::UNIX_EPOCH.into();
    peer.header.authority_association = None;
    peer.header.idempotency_key = raw_key.clone();
    peer.header.ingress_context = None;
    peer.header.retained_resume = None;
    canonical_json(&Input::Peer(peer))
        .map(|bytes| digest(&bytes))
        .map_err(|_| PrivateOriginalError::Conflict)
}
fn origin(request: &PeerIngressBindRequest) -> Result<PeerOrigin, PeerIngressBindRefusal> {
    use meerkat_core::interaction::{
        PeerIngressAuthDecision, PeerIngressAuthExemption, PeerIngressKind,
        PeerIngressSenderAuthentication, PeerInputClass,
    };
    let authentication = |value| match value {
        PeerIngressSenderAuthentication::SignatureVerified => Ok("signature_verified"),
        PeerIngressSenderAuthentication::InProcess => Ok("in_process"),
        _ => Err(PeerIngressBindRefusal::NotOffered),
    };
    Ok(match &request.origin {
        PeerIngressOrigin::Comms(value) => {
            let claim = match &value.claim {
                PeerIngressClaimKind::Durable { raw_item_id } => {
                    serde_json::json!({"durable":raw_item_id})
                }
                PeerIngressClaimKind::Volatile => serde_json::json!({"volatile":true}),
                _ => return Err(PeerIngressBindRefusal::NotOffered),
            };
            let auth = match value.auth {
                PeerIngressAuthDecision::Required => "required",
                PeerIngressAuthDecision::Exempt(PeerIngressAuthExemption::SupervisorBridge) => {
                    "supervisor_bridge"
                }
            };
            let class = match value.class {
                PeerInputClass::ActionableMessage => "actionable_message",
                PeerInputClass::ActionableRequest => "actionable_request",
                PeerInputClass::ResponseProgress => "response_progress",
                PeerInputClass::ResponseTerminal => "response_terminal",
                PeerInputClass::PeerLifecycleAdded => "peer_lifecycle_added",
                PeerInputClass::PeerLifecycleRetired => "peer_lifecycle_retired",
                PeerInputClass::PeerLifecycleUnwired => "peer_lifecycle_unwired",
                PeerInputClass::PeerLifecycleKickoff => "peer_lifecycle_kickoff",
                PeerInputClass::SilentRequest => "silent_request",
                PeerInputClass::Ack => "ack",
                PeerInputClass::PlainEvent => "plain_event",
            };
            let kind = match value.kind {
                PeerIngressKind::Message => "message",
                PeerIngressKind::Request => "request",
                PeerIngressKind::Response => "response",
                PeerIngressKind::Ack => "ack",
                PeerIngressKind::PlainEvent => "plain_event",
            };
            PeerOrigin::Comms {
                sender: value.sender,
                details: serde_json::json!({
                "signing_key":value.signing_key,"route_label":value.route_label,
                "sender_authentication":authentication(value.sender_authentication)?,
                "auth":auth,"class":class,"ingress_kind":kind,
                "interaction_id":value.interaction_id,"in_reply_to":value.in_reply_to,"claim":claim,
                "expected_member":request.expected_member}),
            }
        }
        PeerIngressOrigin::SupervisorDelivery(value) => PeerOrigin::Supervisor {
            details: serde_json::json!({
            "supervisor":value.supervisor,"sender_authentication":authentication(value.sender_authentication)?,
            "interaction_id":value.interaction_id,"tracked":value.tracked,"expected_member":request.expected_member}),
        },
        _ => return Err(PeerIngressBindRefusal::NotOffered),
    })
}
impl PeerSource {
    pub(super) fn refused_sender(&self) -> bool {
        matches!(self.origin, PeerOrigin::Comms { .. }) && self.selection.sender.is_none()
    }
    pub(super) fn event_key(&self) -> IdempotencyKey {
        IdempotencyKey::new(self.event_key.clone())
    }
    pub(super) fn check_input(
        &self,
        input: &Input,
        bound: bool,
    ) -> Result<(), PrivateOriginalError> {
        if input.id() != &self.input_id
            || raw_digest(input, &self.raw_key)? != self.raw_sha256
            || (bound && input.header().idempotency_key.as_ref() != Some(&self.event_key()))
            || input.header().retained_resume.is_some()
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }
    pub(super) fn for_current_offer(
        &self,
        bound: Input,
        offered: Input,
    ) -> Result<Input, PrivateOriginalError> {
        self.check_input(&bound, true)?;
        self.check_input(&offered, false)?;
        // Compare every serialized field, projecting only observation time and
        // the already-bound association into this offer's expected value.
        let mut value = serde_json::to_value(&bound).map_err(|_| PrivateOriginalError::Conflict)?;
        let mut expected =
            serde_json::to_value(&offered).map_err(|_| PrivateOriginalError::Conflict)?;
        let header = expected
            .get_mut("header")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or(PrivateOriginalError::Conflict)?;
        header.insert(
            "timestamp".into(),
            serde_json::to_value(bound.header().timestamp)
                .map_err(|_| PrivateOriginalError::Conflict)?,
        );
        match value.pointer("/header/authority_association") {
            Some(association) => {
                header.insert("authority_association".into(), association.clone());
            }
            None => {
                header.remove("authority_association");
            }
        }
        if value != expected {
            return Err(PrivateOriginalError::Conflict);
        }
        // Custody retains the first exact bytes. Only the returned projection
        // gets this offer's time; serde cannot reconstruct native ingress.
        *value
            .pointer_mut("/header/timestamp")
            .ok_or(PrivateOriginalError::Conflict)? =
            serde_json::to_value(offered.header().timestamp)
                .map_err(|_| PrivateOriginalError::Conflict)?;
        let ingress = bound
            .header()
            .ingress_context
            .as_ref()
            .ok_or(PrivateOriginalError::Conflict)?;
        let projected: Input =
            serde_json::from_value(value).map_err(|_| PrivateOriginalError::Conflict)?;
        projected
            .with_ingress_context((**ingress).clone())
            .map_err(|_| PrivateOriginalError::Conflict)
    }
    fn targets(&self) -> Result<Vec<GrantTarget>, PrivateOriginalError> {
        let mut targets = vec![GrantTarget::Memory {
            scope: self.standing()?.recorder_scope.clone(),
        }];
        if let Some((peer, _)) = &self.selection.sender {
            targets.push(GrantTarget::Peer { peer: *peer });
        }
        targets.sort();
        Ok(targets)
    }
    fn standing(
        &self,
    ) -> Result<
        &crate::installed_host::dynamic_members::MemberStandingOperations,
        PrivateOriginalError,
    > {
        self.selection
            .receiver
            .admission
            .as_ref()
            .and_then(|a| a.standing_operations.as_ref())
            .filter(|s| s.issued.is_some())
            .ok_or(PrivateOriginalError::Denied)
    }
    fn parent(
        &self,
        target: &GrantTarget,
    ) -> Result<
        &meerkat_authorization_contracts::work_association::GrantLineageRef,
        PrivateOriginalError,
    > {
        let standing = self.standing()?;
        let issued = standing
            .issued
            .as_ref()
            .ok_or(PrivateOriginalError::Denied)?;
        match target {
            GrantTarget::Peer { peer }
                if self
                    .selection
                    .sender
                    .as_ref()
                    .is_some_and(|(expected, _)| expected == peer) =>
            {
                Ok(&issued.peer_message)
            }
            GrantTarget::Memory { scope } if scope == &standing.recorder_scope => {
                Ok(&issued.recorder_write)
            }
            _ => Err(PrivateOriginalError::Denied),
        }
    }
    fn values(
        &self,
        application: &PrincipalRef,
        target: &ToolOperationTarget,
    ) -> Result<meerkat_authorization::policy::LocalOperationValues, PrivateOriginalError> {
        let receiver = &self.selection.receiver;
        match target {
            ToolOperationTarget::PeerSend {
                peer,
                kind: PeerSendKind::Message | PeerSendKind::Response,
            } if self
                .selection
                .sender
                .as_ref()
                .is_some_and(|(expected, _)| expected == peer) =>
            {
                let (_, sender) = self
                    .selection
                    .sender
                    .as_ref()
                    .ok_or(PrivateOriginalError::Denied)?;
                Ok(application_operations::peer_operation(
                    application,
                    &receiver.route.logical_executor,
                    &sender.member.audience,
                    peer,
                    false,
                ))
            }
            ToolOperationTarget::MemoryOperation {
                scope,
                operation: MemoryOperation::Write,
            } if scope == &self.standing()?.recorder_scope => {
                application_operations::recorder_operation(
                    application,
                    &receiver.route.logical_executor,
                    &receiver.member.audience,
                    scope,
                    MemoryOperation::Write,
                )
                .ok_or(PrivateOriginalError::Denied)
            }
            _ => Err(PrivateOriginalError::Denied),
        }
    }
    fn leaf_bounds(
        &self,
        application: &PrincipalRef,
        target: &GrantTarget,
    ) -> Result<
        meerkat_authorization_contracts::constraints::ExecutionRestrictions,
        PrivateOriginalError,
    > {
        let value = match target {
            GrantTarget::Peer { peer } => self.values(
                application,
                &ToolOperationTarget::PeerSend {
                    peer: *peer,
                    kind: PeerSendKind::Message,
                },
            )?,
            GrantTarget::Memory { scope } => self.values(
                application,
                &ToolOperationTarget::MemoryOperation {
                    scope: scope.clone(),
                    operation: MemoryOperation::Write,
                },
            )?,
            _ => return Err(PrivateOriginalError::Denied),
        };
        let mut bounds = application_operations::parent_restrictions(value);
        bounds.delegation_depth = DelegationDepth::remaining(0);
        Ok(bounds)
    }
    fn parent_bounds(
        &self,
        application: &PrincipalRef,
        target: &GrantTarget,
    ) -> Result<
        meerkat_authorization_contracts::constraints::ExecutionRestrictions,
        PrivateOriginalError,
    > {
        let mut bounds = self.leaf_bounds(application, target)?;
        bounds.delegation_depth = DelegationDepth::remaining(1);
        if matches!(target, GrantTarget::Peer { .. }) {
            bounds = application_operations::configured_catalog_parent(
                application,
                &self.selection.receiver.route.logical_executor,
                &self.selection.receiver.member.audience,
            );
            bounds.actions = ExactRestriction::exact([ActionRef {
                feature: "toolkit".into(),
                action: "peer-message".into(),
            }]);
            bounds.resource_domains = DomainRestriction::owned_by([application.clone()]);
        }
        Ok(bounds)
    }
    fn evidence(&self) -> Result<PrivateSourceEvidence, PrivateOriginalError> {
        Ok(PrivateSourceEvidence {
            resource_id: self.work.clone(),
            revision: EvidenceId::new("core-peer-ingress-v1")
                .map_err(|_| PrivateOriginalError::Conflict)?,
            sha256: digest(
                &canonical_json(&(
                    &self.origin,
                    &self.raw_sha256,
                    &self.runtime,
                    &self.session,
                    &self.work,
                    &self.event_key,
                ))
                .map_err(|_| PrivateOriginalError::Conflict)?,
            ),
        })
    }
    fn child_id(&self, target: &GrantTarget) -> Result<EvidenceId, PrivateOriginalError> {
        let bytes = canonical_json(&(
            "toolkit.peer-delivery-target.v1",
            &self.work,
            &self.event_key,
            &self.session,
            &self.input_id,
            &self.selection.receiver.admission,
            target,
        ))
        .map_err(|_| PrivateOriginalError::Conflict)?;
        EvidenceId::new(format!("peer-delivery-target/{}", digest(&bytes)))
            .map_err(|_| PrivateOriginalError::Conflict)
    }
}
pub(super) fn validate(record: &SourceRecord) -> Result<(), PrivateOriginalError> {
    let Some(peer) = &record.peer_ingress else {
        return Ok(());
    };
    let receiver = &peer.selection.receiver;
    let standing = peer.standing()?;
    if record.native_operations.is_some()
        || record.late_workflow_helper.is_some()
        || record.class_input.is_some()
        || record.workflow_child.is_some()
        || record.setup_request.is_some()
        || record.configured_setup.is_some()
        || record.approved_task.is_some()
        || record.service_admission.is_some()
        || record.planning_source_evidence.is_some()
        || record.event.original_authentication != peer.evidence()?
        || record.event.root_event != peer.evidence()?
        || !record.event.contributors.is_empty()
        || peer.work != record.event.original
        || peer.event_key != record.event.occurrence.as_str()
        || peer.raw_sha256 != record.event.content_sha256
        || !valid_digest(&peer.raw_sha256)
        || peer.runtime != LogicalRuntimeId::for_session(&peer.session).to_string()
        || receiver.route.id != record.event.route
        || receiver.route.logical_executor != record.source.logical_executor
        || receiver.route.source_authority != record.source.source_authority
        || receiver
            .context
            .as_ref()
            .is_none_or(|c| c.as_str() != peer.session.to_string())
        || standing.recorder_scope
            != (meerkat_mobkit::memory::MemoryScope::Identity {
                realm: record.source.realm.to_string(),
                identity: peer.selection.identity.to_string(),
            })
            .operation_scope_ref()
        || peer.selection.sender.as_ref().is_some_and(|(_, sender)| {
            sender.member.member.mob_id != receiver.member.member.mob_id
                || sender.member.audience != receiver.member.audience
        })
    {
        return Err(PrivateOriginalError::Conflict);
    }
    match (&peer.origin, &peer.selection.sender) {
        (PeerOrigin::Comms { sender, .. }, Some((selected, _))) if sender == selected => (),
        (PeerOrigin::Comms { .. }, None) if record.operation_cancelled => (),
        (PeerOrigin::Supervisor { .. }, None) => (),
        _ => return Err(PrivateOriginalError::Conflict),
    }
    if let Some(association) = &record.final_association {
        check_binding(record, association)?;
    }
    Ok(())
}
pub(super) fn check_binding(
    record: &SourceRecord,
    association: &InputAuthorityAssociation,
) -> Result<(), PrivateOriginalError> {
    let Some(peer) = &record.peer_ingress else {
        return Ok(());
    };
    let c = association.candidate();
    let WorkAuthorityBasis::TargetedGrantLineages {
        base: None,
        targets,
    } = &c.authority_basis
    else {
        return Err(PrivateOriginalError::Conflict);
    };
    if c.original_work.work != peer.work
        || c.target.context.as_str() != peer.session.to_string()
        || c.target.logical_runtime.as_str() != peer.runtime
        || c.target.audience != peer.selection.receiver.member.audience
        || !c.contributing_work.is_empty()
        || targets.len() != peer.targets()?.len()
    {
        return Err(PrivateOriginalError::Conflict);
    }
    for target in peer.targets()? {
        let lineage = c
            .authority_basis
            .lineage_for(Some(&target))
            .ok_or(PrivateOriginalError::Conflict)?;
        if lineage.len() != 2
            || &lineage[0] != peer.parent(&target)?
            || lineage[1].grant_id != peer.child_id(&target)?
            || lineage[0].root_authority != lineage[1].root_authority
        {
            return Err(PrivateOriginalError::Conflict);
        }
    }
    Ok(())
}

struct ReceiverBinder(Weak<PrivateOriginals>);
impl PeerIngressBinder for ReceiverBinder {
    fn bind_peer_ingress(&self, request: PeerIngressBindRequest) -> PeerIngressBindFuture<'_> {
        Box::pin(async move {
            let source = self.0.upgrade().ok_or_else(unavailable)?;
            source.bind_peer_delivery(request).await
        })
    }
}
impl PrivateOriginals {
    pub(crate) fn install_peer_receiver(
        self: &Arc<Self>,
        machine: &Arc<MeerkatMachine>,
    ) -> Result<(), PrivateOriginalError> {
        machine
            .install_peer_ingress_binder(Arc::new(ReceiverBinder(Arc::downgrade(self))))
            .map_err(|_| PrivateOriginalError::Denied)
    }
    fn check_peer_selection(&self, peer: &PeerSource) -> Result<(), PrivateOriginalError> {
        if self.native_selected_member(&peer.selection.receiver.route.id)?
            != peer.selection.receiver
        {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some((_, sender)) = &peer.selection.sender
            && self.native_selected_member(&sender.route.id)? != *sender
        {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(())
    }
    pub(super) fn peer_operation_values(
        &self,
        association: &InputAuthorityAssociation,
        target: &ToolOperationTarget,
    ) -> Result<Option<meerkat_authorization::policy::LocalOperationValues>, PrivateOriginalError>
    {
        let selected = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| held.original.association() == association)
            .and_then(|held| held.peer_ingress.clone());
        let Some(peer) = selected else {
            return Ok(None);
        };
        self.check_peer_selection(&peer)?;
        let context = self.callback_grant_context().map_err(host_error)?;
        peer.values(&context.application, target).map(Some)
    }
    pub(super) async fn peer_operations_basis(
        self: &Arc<Self>,
        reference: &PrivateOriginalRef,
        record: &SourceRecord,
    ) -> Result<WorkAuthorityBasis, PrivateOriginalError> {
        validate(record)?;
        let peer = record
            .peer_ingress
            .clone()
            .ok_or(PrivateOriginalError::Conflict)?;
        self.check_peer_selection(&peer)?;
        if let Some(saved) = &record.final_association {
            self.check_peer_binding(record, saved)
                .map_err(cold_service_admission::writer_error)?;
            return Ok(saved.candidate().authority_basis.clone());
        }
        let owner = self.clone();
        let reference = reference.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || handle.block_on(async {
            let _writer=owner.owner.source_writer().await;
            let row=owner.store.get(&owner.key(&reference)?).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
            let current=owner.decode_record(&row,&reference)?;
            if current.operation_cancelled || current.peer_ingress.as_ref()!=Some(&peer) { return Err(PrivateOriginalError::Denied); }
            let selected=&peer.selection.receiver;
            owner.with_selected_setup_caller(&(selected.route.clone(),selected.member.clone(),selected.context.clone()),
                selected.admission.as_ref(), || {
                owner.check_peer_selection(&peer).map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied)?;
                let context=owner.callback_grant_context()?;
                context.lifetime.with_current_setup(|| {
                    let mut targets=vec![];
                    for target in peer.targets().map_err(denied)? {
                        let parent=peer.parent(&target).map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied)?;

                        if context.grants.issued_reference(&parent.grant_id).map_err(grant_error)?.as_ref()!=Some(parent)
                            || parent.root_authority!=context.operator { return Err(denied(())); }
                        let represented=selected.route.represented_subject.as_ref();
                        let root=context.grants.resolve_lineage(std::slice::from_ref(parent),&context.application,represented).map_err(grant_error)?;
                        let expected=peer.parent_bounds(&context.application,&target).map_err(denied)?;
                        if root.restrictions()!=&expected { return Err(denied(())); }
                        root.check_current().map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable)?;
                        let bounds=peer.leaf_bounds(&context.application,&target).map_err(denied)?;
                        let id=peer.child_id(&target).map_err(denied)?;
                        let child=match context.grants.issued_reference(&id).map_err(grant_error)? {
                            Some(child)=>child,
                            None=>context.grants.issue_child(&context.application,parent,id,selected.route.logical_executor.clone(),bounds.clone()).map_err(grant_error)?,
                        };
                        let lineage=vec![parent.clone(),child];
                        let leaf=context.grants.resolve_lineage(&lineage,&selected.route.logical_executor,represented).map_err(grant_error)?;
                        if leaf.restrictions()!=&bounds { return Err(denied(())); }
                        leaf.check_current().map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable)?;
                        targets.push(TargetGrantLineage { target,lineage });
                    }
                    Ok(WorkAuthorityBasis::TargetedGrantLineages { base:None,targets })
                })
            }).await.map_err(host_error)
        })).await.map_err(|_| PrivateOriginalError::Unavailable)?
    }
    fn check_peer_parents(&self, peer: &PeerSource) -> Result<(), PrivateOriginalError> {
        self.check_peer_selection(peer)?;
        let context = self.callback_grant_context().map_err(host_error)?;
        for target in peer.targets()? {
            let parent = peer.parent(&target)?;
            if parent.root_authority != context.operator
                || context
                    .grants
                    .issued_reference(&parent.grant_id)
                    .map_err(|error| host_error(grant_error(error)))?
                    .as_ref()
                    != Some(parent)
            {
                return Err(PrivateOriginalError::Conflict);
            }
            let root = context
                .grants
                .resolve_lineage(
                    std::slice::from_ref(parent),
                    &context.application,
                    peer.selection.receiver.route.represented_subject.as_ref(),
                )
                .map_err(|error| host_error(grant_error(error)))?;
            if root.restrictions() != &peer.parent_bounds(&context.application, &target)? {
                return Err(PrivateOriginalError::Conflict);
            }
            root.check_current()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
        }
        Ok(())
    }
    /// Reobserve the actual selected roots and leaves under the source writer.
    /// Saved IDs never substitute for issued owner references or current grants.
    pub(super) fn check_peer_binding(
        &self,
        record: &SourceRecord,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let Some(peer) = &record.peer_ingress else {
            return Ok(());
        };
        let checked = || {
            check_binding(record, association)?;
            self.check_peer_selection(peer)?;
            let context = self.callback_grant_context().map_err(host_error)?;
            for target in peer.targets()? {
                let lineage = association
                    .candidate()
                    .authority_basis
                    .lineage_for(Some(&target))
                    .ok_or(PrivateOriginalError::Conflict)?;
                for reference in lineage {
                    if context
                        .grants
                        .issued_reference(&reference.grant_id)
                        .map_err(|error| host_error(grant_error(error)))?
                        .as_ref()
                        != Some(reference)
                    {
                        return Err(PrivateOriginalError::Conflict);
                    }
                }
                if lineage[0].root_authority != context.operator {
                    return Err(PrivateOriginalError::Conflict);
                }
                let leaf = context
                    .grants
                    .resolve_lineage(
                        lineage,
                        &peer.selection.receiver.route.logical_executor,
                        peer.selection.receiver.route.represented_subject.as_ref(),
                    )
                    .map_err(|error| host_error(grant_error(error)))?;
                if leaf.restrictions() != &peer.leaf_bounds(&context.application, &target)? {
                    return Err(PrivateOriginalError::Conflict);
                }
                leaf.check_current()
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
            }
            Ok(())
        };
        checked().map_err(|error| match error {
            PrivateOriginalError::Unavailable => OriginalWriterError::Unavailable,
            PrivateOriginalError::Denied => OriginalWriterError::WrongOwner,
            PrivateOriginalError::Conflict => OriginalWriterError::Conflict,
        })
    }
    pub(super) async fn bind_peer_delivery(
        self: &Arc<Self>,
        request: PeerIngressBindRequest,
    ) -> Result<PeerIngressBinding, PeerIngressBindRefusal> {
        self.with_new_work_admission(|| ())
            .map_err(|_| readiness())?;
        if request.runtime_id != LogicalRuntimeId::for_session(&request.session_id)
            || request.input.header().authority_association.is_some()
            || request.input.header().ingress_context.is_some()
            || request.input.header().retained_resume.is_some()
            || !matches!(request.input, Input::Peer(_))
        {
            return Err(PeerIngressBindRefusal::NotOffered);
        }
        let origin = origin(&request)?;
        let raw = raw_digest(&request.input, &request.input.header().idempotency_key)
            .map_err(|_| unavailable())?;
        // Ordinary Add sources all use the installed ingress authority. This
        // historical key remains readable after its current route retires.
        let key = StoreKey {
            realm: self.scope.realm.clone(),
            owner: self.scope.owner.clone(),
            id: source_key_id(
                &RealmId::parse(&self.scope.realm).map_err(|_| unavailable())?,
                self.owner.ingress_domain(),
                &self.owner.ingress_domain().authority,
                &request.delivery_work,
            )
            .map_err(|_| unavailable())?,
        };
        let existing = self.store.get(&key).await.map_err(|_| unavailable())?;
        let historical = existing
            .as_ref()
            .map(|row| self.decode_stored(row))
            .transpose()
            .map_err(|_| unavailable())?;
        if let Some(record) = &historical {
            let peer = record
                .peer_ingress
                .as_ref()
                .ok_or(PeerIngressBindRefusal::NotOffered)?;
            if peer.session != request.session_id
                || peer.runtime != request.runtime_id.to_string()
                || peer.event_key != request.event_key.to_string()
                || peer.work != request.delivery_work
                || peer.origin != origin
                || peer.raw_sha256 != raw
                || peer.input_id != *request.input.id()
            {
                return Err(PeerIngressBindRefusal::NotOffered);
            }
            if record.operation_cancelled {
                return Err(PeerIngressBindRefusal::Denied);
            }
        }
        let runtime = self.bound_identity_runtime().map_err(|_| readiness())?;
        let policy = self.service_tool_policy.get().ok_or_else(readiness)?;
        let current = policy.member_routes().map_err(|_| readiness())?;
        let receiver = current
            .iter()
            .find(|(a, _)| {
                a.destination
                    .as_ref()
                    .is_some_and(|d| d.context.as_str() == request.session_id.to_string())
            })
            .ok_or_else(readiness)?;
        let selected = self
            .native_selected_member(&receiver.1.id)
            .map_err(|_| readiness())?;
        let admission = selected
            .admission
            .as_ref()
            .ok_or(PeerIngressBindRefusal::NotOffered)?;
        if admission
            .standing_operations
            .as_ref()
            .is_none_or(|s| s.issued.is_none())
        {
            return Err(PeerIngressBindRefusal::NotOffered);
        }
        let identity = runtime
            .owned_identity_for_member_alias(&selected.member.member.member)
            .await
            .ok_or_else(readiness)?;
        self.check_operator_member_route(&selected.member, &selected.route, &identity)
            .map_err(|_| unavailable())?;
        let observed = {
            let mut sender = None;
            if let PeerIngressOrigin::Comms(fact) = &request.origin {
                // Read actual native peer keys; no peer directory tool or
                // sender grant is selected. Every route is host-owned.
                let mut routes = self
                    .routes()
                    .map_err(|_| unavailable())?
                    .iter()
                    .map(|r| r.id.clone())
                    .collect::<Vec<_>>();
                routes.extend(current.iter().map(|(_, r)| r.id.clone()));
                routes.sort();
                routes.dedup();
                for route in routes {
                    let candidate = self
                        .native_selected_member(&route)
                        .map_err(|_| unavailable())?;
                    if candidate.member.member.mob_id != selected.member.member.mob_id {
                        continue;
                    }
                    let id = meerkat_mobkit::member_comms_id::mob_member_id(
                        &candidate.member.member.member,
                    );
                    let entry = runtime
                        .owner()
                        .handle()
                        .get_member(&id)
                        .await
                        .map_err(|_| unavailable())?;
                    if entry.as_ref().and_then(|row| row.peer_id()).as_ref() == Some(&fact.sender) {
                        let entry = entry.ok_or_else(unavailable)?;
                        if sender.is_some()
                            || entry.role.as_str() != candidate.member.member.role
                            || candidate.member.audience != selected.member.audience
                            || candidate.context.as_ref().is_some_and(|c| {
                                entry
                                    .bridge_session_id()
                                    .is_none_or(|s| c.as_str() != s.to_string())
                            })
                        {
                            return Err(PeerIngressBindRefusal::NotOffered);
                        }
                        sender = Some((fact.sender, candidate));
                    }
                }
            }
            PeerSource {
                session: request.session_id.clone(),
                runtime: request.runtime_id.to_string(),
                event_key: request.event_key.to_string(),
                work: request.delivery_work.clone(),
                origin,
                raw_sha256: raw.clone(),
                raw_key: request.input.header().idempotency_key.clone(),
                input_id: request.input.id().clone(),
                selection: PeerSelection {
                    receiver: selected.clone(),
                    identity: identity.clone(),
                    sender,
                },
            }
        };
        let peer = if let Some(record) = historical.as_ref() {
            let saved = record.peer_ingress.as_ref().ok_or_else(unavailable)?;
            if saved != &observed {
                return Err(readiness());
            }
            saved.clone()
        } else {
            observed
        };
        let evidence = peer.evidence().map_err(|_| unavailable())?;
        let event = PrivateAuthenticatedEvent {
            route: peer.selection.receiver.route.id.clone(),
            original: peer.work.clone(),
            occurrence: EvidenceId::new(peer.event_key.clone()).map_err(|_| unavailable())?,
            original_authentication: evidence.clone(),
            root_event: evidence,
            contributors: vec![],
            content_sha256: peer.raw_sha256.clone(),
        };
        let private_route = self.route(&event.route).map_err(|_| readiness())?;
        let reference = self
            .retain_event_for_route_with_peer(event, &private_route, Some(peer.clone()))
            .await
            .map_err(|_| unavailable())?;
        if matches!(peer.origin, PeerOrigin::Comms { .. }) && peer.selection.sender.is_none() {
            return Err(PeerIngressBindRefusal::Denied);
        }
        match self.check_peer_parents(&peer) {
            Ok(()) => (),
            Err(PrivateOriginalError::Denied) => {
                // A definite owner refusal becomes terminal only after the
                // existing canonical cancellation publication is acknowledged.
                self.record_operation_cancelled(&reference)
                    .await
                    .map_err(|_| unavailable())?;
                return Err(PeerIngressBindRefusal::Denied);
            }
            Err(_) => return Err(unavailable()),
        }
        let pins = self
            .controller_pin_source
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(readiness)?;
        let actor = pins
            .live_actor(&request.session_id)
            .await
            .ok_or_else(readiness)?;
        // Meta's captured owner forwards the actual nonwaiting Core pin.
        // Comms never falls back to waiting on the receiving turn.
        let controller = match &request.origin {
            PeerIngressOrigin::Comms(_) => {
                pins.try_pin_actor(&actor).await.map_err(|_| readiness())?
            }
            // Only this origin may wait, within Core's existing 2-second bound.
            PeerIngressOrigin::SupervisorDelivery(_) => {
                pins.pin_actor(&actor).await.map_err(|_| readiness())?
            }
            _ => return Err(PeerIngressBindRefusal::NotOffered),
        };
        let spec = runtime
            .registered_spec(&identity)
            .await
            .map_err(|_| readiness())?
            .spec;
        let context = GovernedBindContext {
            session_id: request.session_id.clone(),
            controller,
            spec,
        };
        let authentication = super::governed_submit::evidence(
            &private_route.authentication_domain,
            &self
                .event(&reference)
                .await
                .map_err(|_| unavailable())?
                .original_authentication,
        )
        .map_err(|_| unavailable())?;
        let binder = Arc::new(super::governed_submit::OriginalBinder {
            review_custody: crate::review_custody::ReviewCustody::default(),
            source: self.clone(),
            grant: None,
            batch: None,
            original: reference,
            held: OnceLock::new(),
            pending: OnceLock::new(),
            authentication,
            event_key: request.event_key,
            console: None,
            approved_login: None,
        });
        let offered = request.input;
        let mut bound = binder
            .bind(&context, offered.clone())
            .await
            .map_err(|_| unavailable())?;
        if bound.entry_fence.is_some() {
            return Err(unavailable());
        }
        self.check_peer_selection(&peer).map_err(|_| readiness())?;
        bound.input = peer
            .for_current_offer(bound.input, offered)
            .map_err(|_| unavailable())?;
        Ok(
            PeerIngressBinding::new(bound.input).with_admission_custody(Box::new(PeerCustody {
                binder,
                settlement: Some(bound.settlement),
            })),
        )
    }
}

/// Transfer only. The existing source recovery worker observes Core's actual
/// admitted row and publishes outside native locks. No synthetic native
/// acknowledgement, source promotion or I/O runs in this native callback.
struct PeerCustody {
    binder: Arc<super::governed_submit::OriginalBinder>,
    settlement: Option<Box<dyn meerkat_mobkit::identity_first::GovernedSettlement>>,
}
impl meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody for PeerCustody {
    fn settle(
        mut self: Box<Self>,
        outcome: meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement,
    ) -> Option<meerkat_runtime::input_admission_custody::NativeInputAdmissionCompletion> {
        // Preserve Core's exact admission fact before transfer releases the
        // foreground and wakes recovery. No source publication occurs here.
        if outcome
            == meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement::Admitted
            && let Some(pending) = self.binder.pending.get()
        {
            pending.handoff.record_native_admitted();
        }
        if let Some(settlement) = self.settlement.take() {
            self.binder.retain_unresolved(
                settlement,
                "native peer admission transferred to source recovery",
            );
        }
        if let Some(pending) = self.binder.pending.get()
            && outcome==meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement::NotAdmitted
        {
            pending.handoff.record_definite_refusal();
        }
        None
    }
}

fn denied<T>(_: T) -> meerkat_toolkit::host_lifetime::control::HostLifetimeError {
    meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied
}
pub(super) fn selected_targets(
    record: &SourceRecord,
) -> Result<Vec<GrantTarget>, PrivateOriginalError> {
    validate(record)?;
    record
        .peer_ingress
        .as_ref()
        .ok_or(PrivateOriginalError::Conflict)?
        .targets()
}

fn grant_error(
    error: meerkat_authorization::grants::GrantRefusal,
) -> meerkat_toolkit::host_lifetime::control::HostLifetimeError {
    use meerkat_authorization::grants::GrantRefusal;
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    match error {
        GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
        _ => HostLifetimeError::Denied,
    }
}
