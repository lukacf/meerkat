//! Restrictive workflow selection under the existing source publication.
//! Actual registration supplies the session. This record grants no access and
//! does not assert that a member or session is currently alive.
use super::*;
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_core::{ControllerRouteKey, MobMemberBinding, RealmId, SessionId};
use meerkat_mob::{MemberDestinationSelection, MemberRuntimeBuildIdentity};
use meerkat_runtime::{identifiers::LogicalRuntimeId, input_authority::NativeAdmissionError};
use meerkat_toolkit::setup::SetupWorkflowRef;

#[derive(Clone)]
pub(crate) struct SelectedWorkflowMember {
    /// Late construction is restricted by the same canonical class admission.
    pub class_admission: Option<Arc<crate::installed_host::dynamic_members::AdmittedMemberClass>>,
    // Pending preparation restricts the exact member/original before final
    // workflow terms exist. Adoption binds this once; it never replaces capture.
    pub reference: Option<SetupWorkflowRef>,
    pub member: MobMemberBinding,
    pub original: OriginalWorkRef,
    pub realm: RealmId,
    pub identity: meerkat_mobkit::identity_first::AgentIdentity,
    pub captured: Option<CapturedWorkflowMember>,
}
#[derive(Clone)]
pub(crate) struct CapturedWorkflowMember {
    pub session: SessionId,
    pub target: NativeWorkTarget,
    pub controller: Option<ControllerRouteKey>,
}

pub(super) fn unavailable() -> NativeAdmissionError {
    NativeAdmissionError::Readiness(ControllerReadinessFailure::PolicyUnavailable)
}
fn denied() -> NativeAdmissionError {
    NativeAdmissionError::Refused(meerkat_core::OperationRefused::new(
        meerkat_core::OperationRefusalKind::Denied,
    ))
}

#[cfg(test)]
mod tests;

impl ToolkitOriginalOwner {
    pub(crate) async fn select_workflow_member(
        &self,
        selected: SelectedWorkflowMember,
        validate_host: impl FnOnce() -> Result<(), OriginalWriterError> + Send,
        select_host: impl FnOnce() -> Result<(), OriginalWriterError> + Send,
    ) -> Result<(), OriginalWriterError> {
        let _mutation = self.mutation.lock().await;
        // Host file/activation observations precede the source publication.
        validate_host()?;
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut members = self
            .workflow_members
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let overlaps = |row: &&SelectedWorkflowMember| {
            row.member == selected.member
                || (selected.reference.is_some() && row.reference == selected.reference)
                || row.original == selected.original
        };
        if members.iter().filter(overlaps).count() > 1 {
            return Err(OriginalWriterError::Conflict);
        }
        if let Some(prior) = members.iter_mut().find(|row| {
            row.member == selected.member
                || (selected.reference.is_some() && row.reference == selected.reference)
                || row.original == selected.original
        }) {
            return if (prior.reference == selected.reference
                || (prior.reference.is_none() && selected.reference.is_some()))
                && prior.member == selected.member
                && prior.original == selected.original
                && prior.realm == selected.realm
                && prior.identity == selected.identity
                && match (&prior.class_admission, &selected.class_admission) {
                    (None, None) => true,
                    (Some(a), Some(b)) => {
                        Arc::ptr_eq(&a.owner, &b.owner) && a.admission_id() == b.admission_id()
                    }
                    _ => false,
                } {
                select_host()?;
                prior.reference = selected.reference;
                Ok(())
            } else {
                Err(OriginalWriterError::Conflict)
            };
        }
        if members.len() >= 128 {
            return Err(OriginalWriterError::Unavailable);
        }
        // The one caller selects only its existing weak host-lifetime slot.
        // All refusal points precede it; no await or fallible step separates
        // that immutable choice from this restrictive member insertion.
        select_host()?;
        members.push(selected);
        Ok(())
    }

    pub(crate) fn capture_workflow_member(
        &self,
        reference: &SetupWorkflowRef,
        identity: &MemberRuntimeBuildIdentity,
        destination: &MemberDestinationSelection,
    ) -> Result<(), OriginalWriterError> {
        self.capture_selected_workflow_member(
            |row| row.reference.as_ref() == Some(reference),
            identity,
            destination,
        )
    }

    pub(crate) fn capture_prepared_workflow_member(
        &self,
        member: &MobMemberBinding,
        original: &OriginalWorkRef,
        identity: &MemberRuntimeBuildIdentity,
        destination: &MemberDestinationSelection,
    ) -> Result<(), OriginalWriterError> {
        self.capture_selected_workflow_member(
            |row| &row.member == member && &row.original == original,
            identity,
            destination,
        )
    }

    pub(crate) fn check_prepared_workflow_member_capture(
        &self,
        member: &MobMemberBinding,
        original: &OriginalWorkRef,
    ) -> Result<(), OriginalWriterError> {
        let members = self
            .workflow_members
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = members
            .iter()
            .find(|row| &row.member == member && &row.original == original)
            .ok_or(OriginalWriterError::WrongOwner)?;
        selected
            .captured
            .as_ref()
            .ok_or(OriginalWriterError::Unavailable)?;
        Ok(())
    }

    fn capture_selected_workflow_member(
        &self,
        matches: impl Fn(&SelectedWorkflowMember) -> bool,
        identity: &MemberRuntimeBuildIdentity,
        destination: &MemberDestinationSelection,
    ) -> Result<(), OriginalWriterError> {
        {
            let members = self
                .workflow_members
                .lock()
                .map_err(|_| OriginalWriterError::Unavailable)?;
            let selected = members
                .iter()
                .find(|row| matches(row))
                .ok_or(OriginalWriterError::WrongOwner)?;
            if identity.member() != &selected.member {
                return Ok(());
            }
        }
        let _mutation = self
            .mutation
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut members = self
            .workflow_members
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = members
            .iter_mut()
            .find(|row| matches(row))
            .ok_or(OriginalWriterError::WrongOwner)?;
        // The wrapper forwards unrelated compiled members unchanged.
        if identity.member() != &selected.member {
            return Ok(());
        }
        if identity.realm_id() != &selected.realm
            || &destination.context != identity.session_document()
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        let session = SessionId::parse(identity.session_document().as_str())
            .map_err(|_| OriginalWriterError::WrongOwner)?;
        let target = NativeWorkTarget {
            logical_owner: destination.logical_owner.clone(),
            logical_runtime: EvidenceId::new(LogicalRuntimeId::for_session(&session).to_string())
                .map_err(|_| OriginalWriterError::WrongOwner)?,
            context: destination.context.clone(),
            context_generation: destination.context_generation,
            audience: destination.audience.clone(),
        };
        if let Some(prior) = &selected.captured {
            return if prior.session == session && prior.target == target {
                Ok(())
            } else {
                Err(OriginalWriterError::Conflict)
            };
        }
        selected.captured = Some(CapturedWorkflowMember {
            session,
            target,
            controller: None,
        });
        Ok(())
    }

    pub(crate) async fn finish_workflow_member(
        &self,
        reference: &SetupWorkflowRef,
        held: &meerkat_mobkit::identity_first::HeldIdentityMember,
    ) -> Result<(), OriginalWriterError> {
        let _mutation = self.mutation.lock().await;
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut members = self
            .workflow_members
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = members
            .iter_mut()
            .find(|row| row.reference.as_ref() == Some(reference))
            .ok_or(OriginalWriterError::WrongOwner)?;
        if selected.identity != held.identity {
            return Err(OriginalWriterError::WrongOwner);
        }
        let key = held
            .controller_route
            .report()
            .ok_or(OriginalWriterError::Unavailable)?
            .key
            .clone();
        let captured = selected
            .captured
            .as_mut()
            .ok_or(OriginalWriterError::Unavailable)?;
        if captured
            .controller
            .as_ref()
            .is_some_and(|prior| prior != &key)
        {
            return Err(OriginalWriterError::Conflict);
        }
        if captured.controller.as_ref() == Some(&key) {
            return Ok(());
        }
        captured.controller = Some(key);
        Ok(())
    }

    /// The caller holds the source writer. Compare the actual registration
    /// capture and native admitted controller without waiting or resolving grants.
    pub(crate) fn check_admitted_workflow_member(
        &self,
        reference: &SetupWorkflowRef,
        member: &MobMemberBinding,
        original: &OriginalWorkRef,
        association: &InputAuthorityAssociation,
        admitted: &meerkat_runtime::meerkat_machine::AdmittedOriginal<'_>,
    ) -> Result<(), OriginalWriterError> {
        let members = self
            .workflow_members
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = members
            .iter()
            .find(|row| row.reference.as_ref() == Some(reference))
            .ok_or(OriginalWriterError::Unavailable)?;
        let captured = selected
            .captured
            .as_ref()
            .ok_or(OriginalWriterError::Unavailable)?;
        if &selected.member != member
            || &selected.original != original
            || selected.realm != association.candidate().ingress_namespace.realm
            || admitted.association() != association
            || admitted.session_id() != &captured.session
            || association.candidate().target != captured.target
            || association.candidate().controller_model.as_ref()
                != Some(admitted.controller().selection())
            || captured.controller.is_some()
        {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(())
    }

    /// Caller holds the source writer after canonical membership validation.
    /// Native supplies the exact current actor's pin. Compare its request-free
    /// route with the source and member, then refresh only the canonical source
    /// pin while publishing any new attachment/interest under the same guards.
    pub(crate) fn complete_restored_workflow_member(
        &self,
        reference: &SetupWorkflowRef,
        member: &MobMemberBinding,
        association: &InputAuthorityAssociation,
        admitted: &meerkat_runtime::meerkat_machine::AdmittedOriginal<'_>,
        expected_route: &ControllerRouteKey,
        // Inspect and, only after publication begins, apply the same exact
        // attachment checks. The callback reports whether its owners differ.
        mut attach: impl FnMut(bool) -> Result<bool, OriginalWriterError>,
    ) -> Result<(), OriginalWriterError> {
        let route = Self::admitted_controller_route(association, admitted)?;
        if &route != expected_route {
            return Err(OriginalWriterError::Conflict);
        }
        let validate = |selected: &SelectedWorkflowMember| -> Result<(), OriginalWriterError> {
            let captured = selected
                .captured
                .as_ref()
                .ok_or(OriginalWriterError::Unavailable)?;
            let claims = association.candidate();
            if &selected.member != member
                || selected.original != claims.original_work
                || selected.realm != claims.ingress_namespace.realm
                || captured.target != claims.target
                || admitted.session_id() != &captured.session
                || captured
                    .controller
                    .as_ref()
                    .is_some_and(|prior| prior != &route)
            {
                return Err(OriginalWriterError::Conflict);
            }
            Ok(())
        };
        let validate_source = |held: &HeldOriginal| -> Result<(), OriginalWriterError> {
            if held.association() != association {
                return Err(OriginalWriterError::Conflict);
            }
            match held.operation {
                OriginalOperationDisposition::Open => (),
                OriginalOperationDisposition::CancellationPending => {
                    return Err(OriginalWriterError::Unavailable);
                }
                OriginalOperationDisposition::Cancelled { .. } => {
                    return Err(OriginalWriterError::OperationCancelled);
                }
            }
            if held.source_custody != Some(SourceCustody::Admitted) {
                return Err(OriginalWriterError::Unavailable);
            }
            let controller = held
                .controller
                .as_ref()
                .ok_or(OriginalWriterError::Unavailable)?;
            if controller.selection() != admitted.controller().selection()
                || controller
                    .plain_facts()
                    .map_err(|_| OriginalWriterError::Unavailable)?
                    .route_key()
                    != route
            {
                return Err(OriginalWriterError::Conflict);
            }
            Ok(())
        };
        // All replay identities match under one healthy observation. A fresh
        // wrapper for this native actor is resource custody, not a policy change.
        let (unchanged, stamp) = self
            .publication
            .observe(|| {
                let members = self
                    .workflow_members
                    .try_lock()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let mut originals = self
                    .originals
                    .try_lock()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let _poison = self.publication.poison_on_unwind();
                let selected = members
                    .iter()
                    .find(|row| row.reference.as_ref() == Some(reference))
                    .ok_or(OriginalWriterError::Unavailable)?;
                validate(selected)?;
                let held = originals
                    .iter_mut()
                    .filter_map(OriginalRecord::bound_mut)
                    .find(|held| held.association() == association)
                    .ok_or(OriginalWriterError::Unavailable)?;
                validate_source(held)?;
                let changed = attach(false)?;
                let unchanged = selected
                    .captured
                    .as_ref()
                    .and_then(|captured| captured.controller.as_ref())
                    == Some(&route)
                    && !changed;
                if unchanged {
                    held.controller = Some(admitted.controller().clone());
                }
                Ok::<_, OriginalWriterError>(unchanged)
            })
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let unchanged = unchanged?;
        stamp
            .check_current()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        if unchanged {
            return Ok(());
        }
        let _publication = self
            .publication
            .try_begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut members = self
            .workflow_members
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let _poison = self.publication.poison_on_unwind();
        let selected = members
            .iter_mut()
            .find(|row| row.reference.as_ref() == Some(reference))
            .ok_or(OriginalWriterError::Unavailable)?;
        validate(selected)?;
        let held = originals
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|held| held.association() == association)
            .ok_or(OriginalWriterError::Unavailable)?;
        validate_source(held)?;
        let captured = selected
            .captured
            .as_mut()
            .ok_or(OriginalWriterError::Unavailable)?;
        attach(true)?;
        // Every fallible check precedes these paired resource/attachment writes.
        held.controller = Some(admitted.controller().clone());
        captured.controller = Some(route);
        Ok(())
    }

    /// Use the same incomplete-controller restriction for operation policy.
    /// A restored source without its workflow attachment must not take the
    /// ordinary singleton policy path. This is no new readiness state.
    pub(crate) fn check_workflow_policy_capture(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let runtime = {
            // Concurrent policy readers share this bounded in-memory owner.
            // The enclosing publication still invalidates intervening mutations.
            let members = self
                .workflow_members
                .lock()
                .map_err(|_| OriginalWriterError::Unavailable)?;
            let claims = association.candidate();
            let selected = members.iter().find(|row| {
                row.original == claims.original_work
                    || row.captured.as_ref().is_some_and(|captured| {
                        captured.target.logical_runtime == claims.target.logical_runtime
                    })
            });
            let Some(selected) = selected else {
                return Ok(());
            };
            let captured = selected
                .captured
                .as_ref()
                .ok_or(OriginalWriterError::Unavailable)?;
            LogicalRuntimeId::for_session(&captured.session)
        };
        self.check_workflow_member(&runtime, association, None)
            .map_err(|error| match error {
                NativeAdmissionError::Refused(_) => OriginalWriterError::OperationDenied,
                _ => OriginalWriterError::Unavailable,
            })
    }

    #[cfg(test)]
    pub(crate) fn with_workflow_policy_capture_locked_for_test<R>(
        &self,
        read: impl FnOnce() -> R,
    ) -> R {
        let _members = self.workflow_members.lock().unwrap();
        read()
    }

    /// A saved row cannot select a different resumed destination. The caller
    /// will separately obtain the native historical admission/controller proof.
    pub(crate) fn check_saved_workflow_destination(
        &self,
        reference: &SetupWorkflowRef,
        member: &MobMemberBinding,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let members = self
            .workflow_members
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = members
            .iter()
            .find(|row| row.reference.as_ref() == Some(reference))
            .ok_or(OriginalWriterError::Unavailable)?;
        let captured = selected
            .captured
            .as_ref()
            .ok_or(OriginalWriterError::Unavailable)?;
        let claims = association.candidate();
        if &selected.member != member
            || selected.original != claims.original_work
            || selected.realm != claims.ingress_namespace.realm
            || captured.target != claims.target
            || captured.session.to_string() != claims.target.context.as_str()
        {
            return Err(OriginalWriterError::Conflict);
        }
        if captured.controller.is_some() {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(())
    }

    /// Read the host's actual selected/captured member for a canonical recovery
    /// row. This does not select a member or establish current native custody.
    /// Both incomplete and completed captures still need native observation.
    pub(crate) fn captured_workflow_member(
        &self,
        reference: &SetupWorkflowRef,
        association: &InputAuthorityAssociation,
    ) -> Result<MobMemberBinding, OriginalWriterError> {
        let members = self
            .workflow_members
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = members
            .iter()
            .find(|row| row.reference.as_ref() == Some(reference))
            .ok_or(OriginalWriterError::Unavailable)?;
        let captured = selected
            .captured
            .as_ref()
            .ok_or(OriginalWriterError::Unavailable)?;
        let claims = association.candidate();
        if selected.original != claims.original_work
            || selected.realm != claims.ingress_namespace.realm
            || captured.target != claims.target
            || captured.session.to_string() != claims.target.context.as_str()
        {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(selected.member.clone())
    }

    /// Return the admission already selected by this exact captured workflow.
    /// This does not create a class admission or change native capture.
    pub(crate) fn captured_workflow_class(
        &self,
        reference: &SetupWorkflowRef,
        association: &InputAuthorityAssociation,
    ) -> Result<Arc<crate::installed_host::dynamic_members::AdmittedMemberClass>, OriginalWriterError>
    {
        let member = self.captured_workflow_member(reference, association)?;
        let members = self
            .workflow_members
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = members
            .iter()
            .find(|row| {
                row.reference.as_ref() == Some(reference)
                    && row.member == member
                    && row.original == association.candidate().original_work
            })
            .ok_or(OriginalWriterError::Unavailable)?;
        selected
            .class_admission
            .clone()
            .ok_or(OriginalWriterError::WrongOwner)
    }

    pub(crate) fn dedicated_workflow_session(
        &self,
        reference: &SetupWorkflowRef,
        member: &MobMemberBinding,
        original: &EvidenceId,
    ) -> Result<Option<SessionId>, OriginalWriterError> {
        let members = self
            .workflow_members
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let Some(selected) = members.iter().find(|row| {
            &row.member == member
                || row.reference.as_ref() == Some(reference)
                || &row.original.work == original
        }) else {
            return Ok(None);
        };
        if selected.reference.as_ref() != Some(reference)
            || &selected.member != member
            || &selected.original.work != original
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        let captured = selected
            .captured
            .as_ref()
            .ok_or(OriginalWriterError::Unavailable)?;
        captured
            .controller
            .as_ref()
            .ok_or(OriginalWriterError::Unavailable)?;
        Ok(Some(captured.session.clone()))
    }

    pub(crate) fn check_workflow_member(
        &self,
        runtime: &LogicalRuntimeId,
        association: &InputAuthorityAssociation,
        controller: Option<&ControllerModelClient>,
    ) -> Result<(), NativeAdmissionError> {
        let members = self.workflow_members.lock().map_err(|_| unavailable())?;
        // This opt-in selects the full roster before bootstrap. Until every
        // destination is captured, an omitted participant must not let an
        // unrelated source bypass the restriction through another gateway.
        // Late class members have exact canonical admission and native capture;
        // only the legacy full-roster selections impose this global barrier.
        if members
            .iter()
            .any(|row| row.captured.is_none() && row.class_admission.is_none())
        {
            return Err(unavailable());
        }
        let claims = association.candidate();
        let selected = members.iter().find(|row| {
            row.original == claims.original_work
                || row.captured.as_ref().is_some_and(|captured| {
                    LogicalRuntimeId::for_session(&captured.session) == *runtime
                })
        });
        let Some(selected) = selected.cloned() else {
            return Ok(());
        };
        drop(members);
        let captured = selected.captured.as_ref().ok_or_else(unavailable)?;
        if let Some(admitted) = &selected.class_admission {
            let current = admitted
                .owner
                .member_route(&admitted.workflow().ok_or_else(denied)?.original.route)
                .map_err(|_| unavailable())?
                .ok_or_else(unavailable)?
                .0;
            if current.id != *admitted.admission_id()
                || current.workflow.as_ref() != admitted.workflow()
                || current.destination.as_ref().is_none_or(|destination| {
                    destination.context != captured.target.context
                        || destination.context_generation != captured.target.context_generation
                })
            {
                return Err(denied());
            }
        }
        let key = captured.controller.as_ref().ok_or_else(unavailable)?;
        if claims.ingress_namespace.realm != selected.realm
            || claims.original_work != selected.original
            || claims.target != captured.target
        {
            return Err(denied());
        }
        if let Some(controller) = controller
            && controller
                .plain_facts()
                .map_err(|_| unavailable())?
                .route_key()
                != *key
        {
            return Err(denied());
        }
        Ok(())
    }
}
