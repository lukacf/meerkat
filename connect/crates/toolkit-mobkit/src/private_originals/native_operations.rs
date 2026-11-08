//! Explicit ordinary-input selections for native comms and identity memory.
//! The source retains intent; native grants and the native operation owners
//! still decide permission and dispatch. No profile or tool name grants access.
use super::*;
use crate::installed_host::application_operations;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::{DelegationDepth, ExactRestriction, LifetimeRestriction},
    work_association::{
        GrantTarget, InputAuthorityAssociation, TargetGrantLineage, WorkAuthorityBasis,
    },
};
use meerkat_core::{MemoryOperation, SessionId, ToolOperationTarget};
use meerkat_toolkit::host_lifetime::{
    control::HostLifetimeError,
    policy::{CompiledNativeSourceRoute, PreparedNativeMember},
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePeerAction {
    Message,
    Request,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRecorderAction {
    Read,
    Write,
    Delete,
}
impl NativeRecorderAction {
    fn operation(self) -> MemoryOperation {
        match self {
            Self::Read => MemoryOperation::Read,
            Self::Write => MemoryOperation::Write,
            Self::Delete => MemoryOperation::Delete,
        }
    }
}

/// Target routes select actual installed members, not arbitrary peer keys or
/// caller-supplied audiences. Recorder scope is this caller's identity only.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeOriginalTarget {
    Peer {
        route: Id,
        actions: Vec<NativePeerAction>,
    },
    PeerDirectory,
    RecorderIdentity {
        actions: Vec<NativeRecorderAction>,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOriginalTargetRequest {
    pub issuance_id: EvidenceId,
    pub target: NativeOriginalTarget,
}

/// Supplied explicitly by the trusted host on its inherited pipe. This is an
/// operator issuance request, never approval inferred from the original text.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOriginalOperationsRequest {
    pub not_before_ms: u64,
    pub expires_at_ms: u64,
    pub targets: Vec<NativeOriginalTargetRequest>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SelectedMember {
    pub(super) route: CompiledNativeSourceRoute,
    pub(super) member: PreparedNativeMember,
    pub(super) context: Option<EvidenceId>,
    pub(super) admission: Option<crate::installed_host::dynamic_members::MemberClassAdmission>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedTarget {
    request: NativeOriginalTargetRequest,
    target: GrantTarget,
    recipient: Option<SelectedMember>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeOperationsSelection {
    request: NativeOriginalOperationsRequest,
    caller: SelectedMember,
    session: SessionId,
    identity: meerkat_mobkit::identity_first::AgentIdentity,
    targets: Vec<SelectedTarget>,
}

fn error(error: HostLifetimeError) -> PrivateOriginalError {
    match error {
        HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
            PrivateOriginalError::Unavailable
        }
        _ => PrivateOriginalError::Denied,
    }
}

fn grant_error(error: GrantRefusal) -> HostLifetimeError {
    match error {
        GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
        _ => HostLifetimeError::Denied,
    }
}

fn sorted_nonempty<T: Ord>(items: &[T]) -> bool {
    !items.is_empty() && items.windows(2).all(|pair| pair[0] < pair[1])
}

impl NativeOriginalOperationsRequest {
    fn validate(&self) -> Result<(), PrivateOriginalError> {
        if self.not_before_ms >= self.expires_at_ms
            || self.expires_at_ms == u64::MAX
            || self.targets.is_empty()
            || self.targets.len() > 32
            || self
                .targets
                .iter()
                .map(|row| &row.issuance_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.targets.len()
            || self.targets.iter().any(|row| match &row.target {
                NativeOriginalTarget::Peer { actions, .. } => !sorted_nonempty(actions),
                NativeOriginalTarget::RecorderIdentity { actions } => !sorted_nonempty(actions),
                NativeOriginalTarget::PeerDirectory => false,
            })
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }
}

impl PrivateOriginals {
    pub(super) fn native_selected_member(
        &self,
        route: &Id,
    ) -> Result<SelectedMember, PrivateOriginalError> {
        let selected = self.selected_source_route(route)?;
        let admission = self.selected_setup_admission(&selected).map_err(error)?;
        Ok(SelectedMember {
            route: selected.0,
            member: selected.1,
            context: selected.2,
            admission,
        })
    }

    /// [`Self::native_selected_member`] for a saved selection's recipient. A
    /// recipient with no process registration at all (Dormant after a
    /// restart) is read from its canonical Active admission, so the check of
    /// a saved selection does not depend on the recipient's residency and
    /// wakes nothing. The caller and every other consumer keep the live read.
    fn native_selected_recipient(
        &self,
        route: &Id,
    ) -> Result<SelectedMember, PrivateOriginalError> {
        let selected = self.recipient_source_route(route)?;
        let admission = self.recipient_setup_admission(&selected).map_err(error)?;
        Ok(SelectedMember {
            route: selected.0,
            member: selected.1,
            context: selected.2,
            admission,
        })
    }

    fn check_native_selection(
        &self,
        selected: &NativeOperationsSelection,
    ) -> Result<(), PrivateOriginalError> {
        if self.native_selected_member(&selected.caller.route.id)? != selected.caller {
            return Err(PrivateOriginalError::Denied);
        }
        for row in &selected.targets {
            if let Some(recipient) = &row.recipient
                && self.native_selected_recipient(&recipient.route.id)? != *recipient
            {
                return Err(PrivateOriginalError::Denied);
            }
        }
        Ok(())
    }

    /// Select before the ordinary first input. The same native preparation
    /// captures the actual caller session/controller without submitting input.
    /// Exact retries retain the original selection; they cannot retarget it.
    pub async fn prepare_original_native_operations(
        self: &Arc<Self>,
        submission: PrivateOriginalSubmit,
        request: NativeOriginalOperationsRequest,
    ) -> Result<(), PrivateOriginalError> {
        request.validate()?;
        let now = u64::try_from(self.store.now_ms().await.map_err(store_error)?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if now >= request.expires_at_ms {
            return Err(PrivateOriginalError::Denied);
        }
        let prepared = self.prepare_local_controller_setup(submission).await?;
        let reference = prepared.original();
        let key = self.key(reference)?;
        let row = self
            .store
            .get(&key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        if let Some(saved) = &record.native_operations {
            if saved.request != request || saved.session != *prepared.session_id() {
                return Err(PrivateOriginalError::Conflict);
            }
            self.check_native_selection(saved)?;
            return if record.operation_cancelled {
                Err(PrivateOriginalError::Denied)
            } else {
                Ok(())
            };
        }
        if record.operation_cancelled
            || record.final_association.is_some()
            || record.planning_source_evidence.is_some()
            || record.approved_task.is_some()
            || record.service_admission.is_some()
            || record.class_input.is_some()
            || record.configured_setup.is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let context = self.callback_grant_context().map_err(error)?;
        let caller = self.native_selected_member(&reference.route)?;
        let native = context
            .lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let runtime = self.bound_identity_runtime()?;
        let identity = runtime
            .owned_identity_for_member_alias(&caller.member.member.member)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let mut targets = Vec::new();
        for requested in &request.targets {
            let (target, recipient) = match &requested.target {
                NativeOriginalTarget::Peer { route, .. } => {
                    let selected = self.native_selected_member(route)?;
                    if selected.member.member.mob_id != caller.member.member.mob_id {
                        return Err(PrivateOriginalError::Denied);
                    }
                    let id = meerkat_mobkit::member_comms_id::mob_member_id(
                        &selected.member.member.member,
                    );
                    let entry = runtime
                        .owner()
                        .handle()
                        .get_member(&id)
                        .await
                        .map_err(|_| PrivateOriginalError::Unavailable)?
                        .ok_or(PrivateOriginalError::Denied)?;
                    if entry.role.as_str() != selected.member.member.role
                        || selected.context.as_ref().is_some_and(|context| {
                            entry
                                .bridge_session_id()
                                .is_none_or(|session| context.as_str() != session.to_string())
                        })
                    {
                        return Err(PrivateOriginalError::Denied);
                    }
                    let peer = entry.peer_id().ok_or(PrivateOriginalError::Unavailable)?;
                    (GrantTarget::Peer { peer }, Some(selected))
                }
                NativeOriginalTarget::PeerDirectory => (GrantTarget::PeerDirectory, None),
                NativeOriginalTarget::RecorderIdentity { .. } => (
                    GrantTarget::Memory {
                        scope: meerkat_mobkit::memory::records::MemoryScope::Identity {
                            realm: native.realm.to_string(),
                            identity: identity.to_string(),
                        }
                        .operation_scope_ref(),
                    },
                    None,
                ),
            };
            targets.push(SelectedTarget {
                request: requested.clone(),
                target,
                recipient,
            });
        }
        targets.sort_by(|a, b| a.target.cmp(&b.target));
        if !sorted_nonempty(&targets.iter().map(|row| &row.target).collect::<Vec<_>>()) {
            return Err(PrivateOriginalError::Conflict);
        }
        let selection = NativeOperationsSelection {
            request,
            caller,
            session: prepared.session_id().clone(),
            identity,
            targets,
        };
        self.check_native_selection(&selection)?;
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: record.source.source_authority.clone(),
            work: record.event.original.clone(),
        };
        self.owner
            .with_unbound_source_selection(&record.source.realm, &original, || async {
                let stored = self
                    .store
                    .get(&key)
                    .await
                    .map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let mut current = self.decode_record(&stored, reference)?;
                self.check_native_selection(&selection)?;
                context
                    .lifetime
                    .validate_current_selection()
                    .map_err(error)?;
                if current != record {
                    return Err(PrivateOriginalError::Conflict);
                }
                current.native_operations = Some(selection);
                self.store
                    .compare_and_swap(Transition {
                        key,
                        expected_revision: stored.revision,
                        expected_digest: Some(stored.content_digest),
                        claim: None,
                        data: encode(current)?,
                        events: vec![],
                        effects: vec![],
                    })
                    .await
                    .map_err(store_error)?;
                Ok(())
            })
            .await
            .map_err(|err| match err {
                OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
                OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
                _ => PrivateOriginalError::Unavailable,
            })?
    }

    /// The source binding owns this immutable selection. Current policy reads
    /// do not load grants from it: native selects and resolves the exact target.
    pub(crate) fn native_operation_values(
        &self,
        association: &InputAuthorityAssociation,
        target: &ToolOperationTarget,
    ) -> Result<meerkat_authorization::policy::LocalOperationValues, PrivateOriginalError> {
        if let Some(value) = self.peer_operation_values(association, target)? {
            return Ok(value);
        }
        let selected = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| held.original.association() == association)
            .and_then(|held| held.native_operations.clone())
            .ok_or(PrivateOriginalError::Denied)?;
        self.check_native_selection(&selected)?;
        let context = self.callback_grant_context().map_err(error)?;
        let row = selected
            .targets
            .iter()
            .find(|row| match (&row.target, target) {
                (
                    GrantTarget::Peer { peer: expected },
                    ToolOperationTarget::PeerSend { peer, .. },
                ) => expected == peer,
                (GrantTarget::PeerDirectory, ToolOperationTarget::PeerDirectoryRead) => true,
                (
                    GrantTarget::Memory { scope: expected },
                    ToolOperationTarget::MemoryOperation { scope, .. },
                ) => expected == scope,
                _ => false,
            })
            .ok_or(PrivateOriginalError::Denied)?;
        row.values(&context.application, &selected, Some(target))?
            .into_iter()
            .next()
            .ok_or(PrivateOriginalError::Denied)
    }
}

impl SelectedTarget {
    fn values(
        &self,
        application: &PrincipalRef,
        selection: &NativeOperationsSelection,
        actual: Option<&ToolOperationTarget>,
    ) -> Result<Vec<meerkat_authorization::policy::LocalOperationValues>, PrivateOriginalError>
    {
        let caller = &selection.caller;
        let executor = &caller.route.logical_executor;
        match (&self.request.target, &self.target) {
            (NativeOriginalTarget::Peer { actions, .. }, GrantTarget::Peer { peer }) => {
                let audience = &self
                    .recipient
                    .as_ref()
                    .ok_or(PrivateOriginalError::Conflict)?
                    .member
                    .audience;
                let selected = match actual {
                    Some(ToolOperationTarget::PeerSend { kind, .. }) => {
                        let action = match kind {
                            meerkat_core::PeerSendKind::Message
                            | meerkat_core::PeerSendKind::Response => NativePeerAction::Message,
                            meerkat_core::PeerSendKind::Request => NativePeerAction::Request,
                            _ => return Err(PrivateOriginalError::Denied),
                        };
                        if !actions.contains(&action) {
                            return Err(PrivateOriginalError::Denied);
                        }
                        vec![action]
                    }
                    None => actions.clone(),
                    _ => return Err(PrivateOriginalError::Denied),
                };
                Ok(selected
                    .into_iter()
                    .map(|action| {
                        application_operations::peer_operation(
                            application,
                            executor,
                            audience,
                            peer,
                            action == NativePeerAction::Request,
                        )
                    })
                    .collect())
            }
            (NativeOriginalTarget::PeerDirectory, GrantTarget::PeerDirectory) => {
                Ok(vec![application_operations::peer_directory(
                    application,
                    executor,
                    &caller.member.audience,
                    &selection.session,
                )])
            }
            (NativeOriginalTarget::RecorderIdentity { actions }, GrantTarget::Memory { scope }) => {
                let selected = match actual {
                    Some(ToolOperationTarget::MemoryOperation { operation, .. }) => {
                        let action = actions
                            .iter()
                            .find(|action| action.operation() == *operation)
                            .ok_or(PrivateOriginalError::Denied)?;
                        vec![*action]
                    }
                    None => actions.clone(),
                    _ => return Err(PrivateOriginalError::Denied),
                };
                selected
                    .into_iter()
                    .map(|action| {
                        application_operations::recorder_operation(
                            application,
                            executor,
                            &caller.member.audience,
                            scope,
                            action.operation(),
                        )
                        .ok_or(PrivateOriginalError::Denied)
                    })
                    .collect()
            }
            _ => Err(PrivateOriginalError::Conflict),
        }
    }
}

fn child_id(
    record: &SourceRecord,
    target: &SelectedTarget,
) -> Result<EvidenceId, PrivateOriginalError> {
    let bytes = canonical_json(&(
        "toolkit.native-original-target.v1",
        &record.source,
        &record.event,
        &target.request.issuance_id,
        &target.target,
    ))
    .map_err(|_| PrivateOriginalError::Conflict)?;
    EvidenceId::new(format!("native-original-target/{}", digest(&bytes)))
        .map_err(|_| PrivateOriginalError::Conflict)
}

pub(super) fn validate(record: &SourceRecord) -> Result<(), PrivateOriginalError> {
    let Some(selection) = &record.native_operations else {
        return Ok(());
    };
    selection.request.validate()?;
    if record.planning_source_evidence.is_some()
        || record.approved_task.is_some()
        || record.service_admission.is_some()
        || selection.caller.route.id != record.event.route
        || selection.caller.route.logical_executor != record.source.logical_executor
        || selection.caller.route.source_authority != record.source.source_authority
        || selection.targets.len() != selection.request.targets.len()
        || !sorted_nonempty(
            &selection
                .targets
                .iter()
                .map(|row| &row.target)
                .collect::<Vec<_>>(),
        )
    {
        return Err(PrivateOriginalError::Conflict);
    }
    for selected in &selection.targets {
        if !selection.request.targets.contains(&selected.request) {
            return Err(PrivateOriginalError::Conflict);
        }
        match (
            &selected.request.target,
            &selected.target,
            &selected.recipient,
        ) {
            (
                NativeOriginalTarget::Peer { route, .. },
                GrantTarget::Peer { .. },
                Some(recipient),
            ) if route == &recipient.route.id
                && recipient.member.member.mob_id == selection.caller.member.member.mob_id => {}
            (NativeOriginalTarget::PeerDirectory, GrantTarget::PeerDirectory, None) => {}
            (
                NativeOriginalTarget::RecorderIdentity { .. },
                GrantTarget::Memory { scope },
                None,
            ) if scope
                == &(meerkat_mobkit::memory::records::MemoryScope::Identity {
                    realm: record.source.realm.to_string(),
                    identity: selection.identity.to_string(),
                })
                .operation_scope_ref() => {}
            _ => return Err(PrivateOriginalError::Conflict),
        }
    }
    Ok(())
}

pub(super) fn check_binding(
    record: &SourceRecord,
    association: &InputAuthorityAssociation,
) -> Result<(), PrivateOriginalError> {
    let Some(selection) = &record.native_operations else {
        return Ok(());
    };
    validate(record)?;
    let claims = association.candidate();
    if claims.target.context.as_str() != selection.session.to_string()
        || claims.logical_executor != selection.caller.route.logical_executor
        || claims.target.audience != selection.caller.member.audience
    {
        return Err(PrivateOriginalError::Conflict);
    }
    let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = &claims.authority_basis else {
        return Err(PrivateOriginalError::Conflict);
    };
    for selected in &selection.targets {
        let lineage = claims
            .authority_basis
            .lineage_for(Some(&selected.target))
            .ok_or(PrivateOriginalError::Conflict)?;
        if lineage.len() != 2
            || lineage[0].grant_id != selected.request.issuance_id
            || lineage[1].grant_id != child_id(record, selected)?
            || lineage[0].root_authority != lineage[1].root_authority
        {
            return Err(PrivateOriginalError::Conflict);
        }
    }
    // No peer or Recorder target can enter through a different selection.
    if targets.iter().any(|row| {
        matches!(
            row.target,
            GrantTarget::Peer { .. } | GrantTarget::PeerDirectory | GrantTarget::Memory { .. }
        ) && !selection
            .targets
            .iter()
            .any(|selected| selected.target == row.target)
    }) {
        return Err(PrivateOriginalError::Conflict);
    }
    Ok(())
}

/// Validate the independent existing setup/class selection with only its exact
/// basis. The full source and native rows are checked before this projection.
pub(super) fn base_projection(
    record: &SourceRecord,
    association: &InputAuthorityAssociation,
) -> Result<Option<(SourceRecord, InputAuthorityAssociation)>, PrivateOriginalError> {
    let Some(selection) = &record.native_operations else {
        return Ok(None);
    };
    check_binding(record, association)?;
    let mut claims = association.candidate().clone();
    let WorkAuthorityBasis::TargetedGrantLineages { base, targets } = &claims.authority_basis
    else {
        return Err(PrivateOriginalError::Conflict);
    };
    let targets = targets
        .iter()
        .filter(|row| {
            !selection
                .targets
                .iter()
                .any(|native| native.target == row.target)
        })
        .cloned()
        .collect::<Vec<_>>();
    let simple_setup = record
        .setup_request
        .as_ref()
        .is_some_and(|selected| selected.helper_targets.is_empty())
        || record.configured_setup.as_ref().is_some_and(|selected| {
            selected.capability.helper_targets.is_empty()
                && selected.capability.workflow_targets.is_empty()
        });
    claims.authority_basis = if simple_setup
        && base.is_none()
        && targets.len() == 1
        && targets[0].target == crate::installed_host::operator_setup::setup_targets::find_target()
    {
        WorkAuthorityBasis::GrantLineage {
            lineage: targets[0].lineage.clone(),
        }
    } else {
        WorkAuthorityBasis::TargetedGrantLineages {
            base: base.clone(),
            targets,
        }
    };
    let projected =
        InputAuthorityAssociation::new(claims).map_err(|_| PrivateOriginalError::Conflict)?;
    let mut record = record.clone();
    if record
        .final_association
        .as_ref()
        .is_some_and(|saved| saved != association)
    {
        return Err(PrivateOriginalError::Conflict);
    }
    record.final_association = record.final_association.as_ref().map(|_| projected.clone());
    record.native_operations = None;
    Ok(Some((record, projected)))
}

impl PrivateOriginals {
    /// The canonical source writer calls this before first publication. Native
    /// references are re-observed, not reconstructed from their identifiers.
    pub(super) fn check_native_binding(
        &self,
        record: &SourceRecord,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let Some(selection) = &record.native_operations else {
            return Ok(());
        };
        let checked = || {
            check_binding(record, association)?;
            let context = self.callback_grant_context().map_err(error)?;
            if record.final_association.is_none() {
                self.check_native_selection(selection)?;
            }
            for selected in &selection.targets {
                let lineage = association
                    .candidate()
                    .authority_basis
                    .lineage_for(Some(&selected.target))
                    .ok_or(PrivateOriginalError::Conflict)?;
                for reference in lineage {
                    if context
                        .grants
                        .issued_reference(&reference.grant_id)
                        .map_err(|e| error(grant_error(e)))?
                        .as_ref()
                        != Some(reference)
                    {
                        return Err(PrivateOriginalError::Conflict);
                    }
                }
                if lineage[0].root_authority != context.operator {
                    return Err(PrivateOriginalError::Conflict);
                }
                if record.final_association.is_none() {
                    let values = selected.values(&context.application, selection, None)?;
                    let mut bounds = application_operations::parent_restrictions(
                        values
                            .first()
                            .cloned()
                            .ok_or(PrivateOriginalError::Conflict)?,
                    );
                    bounds.actions =
                        ExactRestriction::exact(values.into_iter().map(|value| value.action));
                    bounds.lifetime = LifetimeRestriction::window(
                        selection.request.not_before_ms,
                        selection.request.expires_at_ms,
                    );
                    bounds.delegation_depth = DelegationDepth::remaining(0);
                    let leaf = context
                        .grants
                        .resolve_lineage(
                            lineage,
                            &selection.caller.route.logical_executor,
                            selection.caller.route.represented_subject.as_ref(),
                        )
                        .map_err(|e| error(grant_error(e)))?;
                    if leaf.restrictions() != &bounds {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    leaf.check_current()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                }
            }
            Ok(())
        };
        checked().map_err(|err| match err {
            PrivateOriginalError::Unavailable => OriginalWriterError::Unavailable,
            PrivateOriginalError::Denied => OriginalWriterError::WrongOwner,
            PrivateOriginalError::Conflict => OriginalWriterError::Conflict,
        })
    }

    /// The existing source writer and current member exclusion guard effect
    /// entry. Native IDs reconcile lost issuance replies; no fresh ID is chosen.
    pub(super) async fn native_operations_basis(
        self: &Arc<Self>,
        reference: &PrivateOriginalRef,
        expected: &SourceRecord,
        session: &SessionId,
        basis: WorkAuthorityBasis,
    ) -> Result<WorkAuthorityBasis, PrivateOriginalError> {
        let Some(selection) = expected.native_operations.clone() else {
            return Ok(basis);
        };
        if &selection.session != session {
            return Err(PrivateOriginalError::Denied);
        }
        self.check_native_selection(&selection)?;
        let stored = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        if record.native_operations.as_ref() != Some(&selection) || record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        let context = self.callback_grant_context().map_err(error)?;
        let extra = if let Some(saved) = &record.final_association {
            check_binding(&record, saved)?;
            selection
                .targets
                .iter()
                .map(|selected| TargetGrantLineage {
                    target: selected.target.clone(),
                    lineage: saved
                        .candidate()
                        .authority_basis
                        .lineage_for(Some(&selected.target))
                        .expect("checked exact target")
                        .to_vec(),
                })
                .collect::<Vec<_>>()
        } else {
            // The member policy owner uses one exclusion for its whole census.
            // Hold it once even when both caller and recipient are dynamic.
            let guard = std::iter::once(&selection.caller)
                .chain(
                    selection
                        .targets
                        .iter()
                        .filter_map(|row| row.recipient.as_ref()),
                )
                .find(|member| member.admission.is_some())
                .unwrap_or(&selection.caller)
                .clone();
            let owner = self.clone();
            let handle = tokio::runtime::Handle::current();
            let issue_selection = selection.clone();
            let reference = reference.clone();
            tokio::task::spawn_blocking(move || {
                handle.block_on(async {
                    // The task retains the source writer even if its observer
                    // drops while waiting for the member exclusion.
                    let _writer = owner.owner.source_writer().await;
                    let row = owner
                        .store
                        .get(
                            &owner
                                .key(&reference)
                                .map_err(|_| HostLifetimeError::Denied)?,
                        )
                        .await
                        .map_err(|_| HostLifetimeError::Unavailable)?
                        .ok_or(HostLifetimeError::Unavailable)?;
                    let record = owner
                        .decode_record(&row, &reference)
                        .map_err(|_| HostLifetimeError::Denied)?;
                    if record.operation_cancelled
                        || record.native_operations.as_ref() != Some(&issue_selection)
                    {
                        return Err(HostLifetimeError::Denied);
                    }
                    owner
                        .with_selected_setup_caller(
                            &(
                                guard.route.clone(),
                                guard.member.clone(),
                                guard.context.clone(),
                            ),
                            guard.admission.as_ref(),
                            || {
                                owner
                                    .check_native_selection(&issue_selection)
                                    .map_err(|err| match err {
                                        PrivateOriginalError::Unavailable => {
                                            HostLifetimeError::Unavailable
                                        }
                                        _ => HostLifetimeError::Denied,
                                    })?;
                                context.lifetime.with_current_setup(|| {
                                    issue_selection
                                        .targets
                                        .iter()
                                        .map(|selected| {
                                            let values = selected
                                                .values(
                                                    &context.application,
                                                    &issue_selection,
                                                    None,
                                                )
                                                .map_err(|_| HostLifetimeError::Denied)?;
                                            let mut bounds =
                                                application_operations::parent_restrictions(
                                                    values
                                                        .first()
                                                        .cloned()
                                                        .ok_or(HostLifetimeError::Denied)?,
                                                );
                                            bounds.actions = ExactRestriction::exact(
                                                values.into_iter().map(|value| value.action),
                                            );
                                            bounds.lifetime = LifetimeRestriction::window(
                                                issue_selection.request.not_before_ms,
                                                issue_selection.request.expires_at_ms,
                                            );
                                            let grants = &context.grants;
                                            let represented = issue_selection
                                                .caller
                                                .route
                                                .represented_subject
                                                .as_ref();
                                            let root = match grants
                                                .issued_reference(&selected.request.issuance_id)
                                                .map_err(grant_error)?
                                            {
                                                Some(root) => root,
                                                None => grants
                                                    .issue_root(
                                                        &context.operator,
                                                        selected.request.issuance_id.clone(),
                                                        context.application.clone(),
                                                        represented.cloned(),
                                                        bounds.clone(),
                                                    )
                                                    .map_err(grant_error)?,
                                            };
                                            if root.root_authority != context.operator {
                                                return Err(HostLifetimeError::Denied);
                                            }
                                            let parent = grants
                                                .resolve_lineage(
                                                    std::slice::from_ref(&root),
                                                    &context.application,
                                                    represented,
                                                )
                                                .map_err(grant_error)?;
                                            if parent.restrictions() != &bounds {
                                                return Err(HostLifetimeError::Denied);
                                            }
                                            parent
                                                .check_current()
                                                .map_err(|_| HostLifetimeError::Unavailable)?;
                                            bounds.delegation_depth = DelegationDepth::remaining(0);
                                            let id = child_id(&record, selected)
                                                .map_err(|_| HostLifetimeError::Denied)?;
                                            let child = match grants
                                                .issued_reference(&id)
                                                .map_err(grant_error)?
                                            {
                                                Some(child) => child,
                                                None => grants
                                                    .issue_child(
                                                        &context.application,
                                                        &root,
                                                        id,
                                                        issue_selection
                                                            .caller
                                                            .route
                                                            .logical_executor
                                                            .clone(),
                                                        bounds.clone(),
                                                    )
                                                    .map_err(grant_error)?,
                                            };
                                            let lineage = vec![root, child];
                                            let leaf = grants
                                                .resolve_lineage(
                                                    &lineage,
                                                    &issue_selection.caller.route.logical_executor,
                                                    represented,
                                                )
                                                .map_err(grant_error)?;
                                            if leaf.restrictions() != &bounds {
                                                return Err(HostLifetimeError::Denied);
                                            }
                                            leaf.check_current()
                                                .map_err(|_| HostLifetimeError::Unavailable)?;
                                            Ok(TargetGrantLineage {
                                                target: selected.target.clone(),
                                                lineage,
                                            })
                                        })
                                        .collect::<Result<Vec<_>, HostLifetimeError>>()
                                })
                            },
                        )
                        .await
                })
            })
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .map_err(error)?
        };
        let (base, mut targets) = match basis {
            WorkAuthorityBasis::TargetedGrantLineages { base, targets } => (base, targets),
            WorkAuthorityBasis::GrantLineage { lineage } => (
                None,
                vec![TargetGrantLineage {
                    target: crate::installed_host::operator_setup::setup_targets::find_target(),
                    lineage,
                }],
            ),
            _ => return Err(PrivateOriginalError::Denied),
        };
        // An exact bound replay may already contain these same native rows.
        // Remove only byte-equivalent selected rows before composing the basis.
        for existing in targets.iter().filter(|row| {
            selection
                .targets
                .iter()
                .any(|selected| selected.target == row.target)
        }) {
            if !extra.contains(existing) {
                return Err(PrivateOriginalError::Conflict);
            }
        }
        targets.retain(|row| {
            !selection
                .targets
                .iter()
                .any(|selected| selected.target == row.target)
        });
        targets.extend(extra);
        targets.sort_by(|a, b| a.target.cmp(&b.target));
        if targets.len()
            > meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES
            || !sorted_nonempty(&targets.iter().map(|row| &row.target).collect::<Vec<_>>())
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(WorkAuthorityBasis::TargetedGrantLineages { base, targets })
    }
}

pub(super) fn selected_targets(record: &SourceRecord) -> Vec<GrantTarget> {
    record
        .native_operations
        .as_ref()
        .map_or_else(Vec::new, |selection| {
            selection
                .targets
                .iter()
                .map(|row| row.target.clone())
                .collect()
        })
}
