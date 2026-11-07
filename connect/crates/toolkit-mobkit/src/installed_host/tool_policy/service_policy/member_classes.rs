//! Concrete class decisions share the installed application's existing policy
//! slot and exclusion. Native identity, held-add and roster remain native-owned.
use super::*;
use crate::installed_host::dynamic_members::{
    AdmittedMemberClass, ApplicationMemberPolicyRequest, MemberClassAddToken, MemberClassAdmission,
    MemberClassAdmissionPhase, MemberClassAdmissionRequest, MemberClassCheckpoint,
    MemberClassDestination, MemberClassObservation, MemberClassWorkflowBinding,
    PreparedMemberClassTools, native_surface_identities,
};
use meerkat_core::{MobMemberBinding, PreparedToolSurface, RealmId};
use meerkat_mob::{MemberDestinationSelection, MemberRuntimeBuildIdentity};
use meerkat_runtime::RuntimeSessionRegistrationWitness;

// Test-only observation. It is declared before the canonical guards so its
// drop writes after they release, including a refused or cancelled await.
#[cfg(test)]
struct MemberPreparationDiagnostic {
    stage: &'static str,
    completed: bool,
    expected: Option<(usize, String)>,
    actual: Option<(usize, String)>,
}
#[cfg(test)]
impl MemberPreparationDiagnostic {
    fn new() -> Self {
        Self {
            stage: "destination",
            completed: false,
            expected: None,
            actual: None,
        }
    }
    fn surfaces(
        &mut self,
        expected: Option<&[meerkat_core::ToolIdentity]>,
        actual: Option<&[meerkat_core::ToolIdentity]>,
    ) {
        use sha2::{Digest, Sha256};
        fn fingerprint(surface: &[meerkat_core::ToolIdentity]) -> Option<(usize, String)> {
            serde_json::to_vec(surface)
                .ok()
                .map(|bytes| (surface.len(), format!("{:x}", Sha256::digest(bytes))))
        }
        self.expected = expected.and_then(fingerprint);
        self.actual = actual.and_then(fingerprint);
    }
}
#[cfg(test)]
impl Drop for MemberPreparationDiagnostic {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        use std::io::Write as _;
        let observation = serde_json::json!({
            "event": "synthetic_member_preparation_incomplete",
            "stage": self.stage,
            "expected_count": self.expected.as_ref().map(|value| value.0),
            "expected_sha256": self.expected.as_ref().map(|value| value.1.as_str()),
            "actual_count": self.actual.as_ref().map(|value| value.0),
            "actual_sha256": self.actual.as_ref().map(|value| value.1.as_str()),
        });
        let _ = writeln!(std::io::stderr(), "{observation}");
    }
}

pub(super) struct MemberRegistrationSelection {
    admission: toolkit_package::Id,
    destination: MemberClassDestination,
    registration: RuntimeSessionRegistrationWitness,
    // Process evidence from actual surface verification or native Created.
    // Equal logical destinations never substitute for this exact witness.
    verified: Option<RuntimeSessionRegistrationWitness>,
}

/// Current canonical coordinates for an ordinary source whose process has not
/// registered an actor yet. This is not a live destination or grant selection.
/// Only this owner constructs it; its route is never published as live evidence.
pub(crate) struct ColdOrdinaryMemberSourceCoordinates {
    admission: MemberClassAdmission,
    route: meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
}
impl ColdOrdinaryMemberSourceCoordinates {
    pub(crate) fn admission(&self) -> &MemberClassAdmission {
        &self.admission
    }
    pub(crate) fn route(
        &self,
    ) -> &meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute {
        &self.route
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MemberPolicyDecision {
    admission: MemberClassAdmission,
    prior: String,
    candidate: String,
}

impl ServicePolicyOwner {
    /// Keep the existing canonical exclusion and application reservation over
    /// a synchronous external authority operation. The expected immutable
    /// admission is not a copied lifecycle or a substitute for this read.
    pub(crate) async fn with_current_member<T>(
        &self,
        id: &toolkit_package::Id,
        class: &meerkat_toolkit::host_lifetime::policy::CompiledMemberClass,
        instance: &meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance,
        effect: impl FnOnce() -> Result<T, HostLifetimeError>,
    ) -> Result<T, HostLifetimeError> {
        let _exclusion = self.exclusion.lock().await;
        let (_, slot) = self.read().await?;
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        let current = slot
            .admissions
            .iter()
            .find(|row| &row.id == id)
            .ok_or(HostLifetimeError::Denied)?;
        if &current.class != class
            || &current.instance != instance
            || current.phase == MemberClassAdmissionPhase::Retired
        {
            return Err(HostLifetimeError::Denied);
        }
        let policy = slot
            .decision
            .as_ref()
            .map(|decision| {
                self.recovery_reservation(
                    decision,
                    decision.proposal.candidate_canonical_json.as_bytes(),
                    &slot,
                )
            })
            .transpose()?;
        let member = self.reserve_member(current, &slot.current)?;
        if let Some(policy) = &policy {
            policy.check_current()?;
        }
        member.check_current()?;
        effect()
    }

    pub(crate) fn callback_bundle(
        &self,
        admission: &MemberClassAdmission,
    ) -> Result<String, HostLifetimeError> {
        self.validate_admission(admission)?;
        crate::installed_host::dynamic_members::callbacks::bundle(
            &self.key.realm,
            &self.key.owner,
            admission,
        )
    }
    fn member_realm(&self) -> Result<RealmId, HostLifetimeError> {
        RealmId::parse(&self.key.realm).map_err(|_| invalid())
    }
    fn validate_admission(
        &self,
        admission: &MemberClassAdmission,
    ) -> Result<(), HostLifetimeError> {
        if !self.classes.contains(&admission.class)
            || admission.instance.member.setup_capability.is_some()
            || admission.class.purpose.is_ordinary() != admission.workflow.is_none()
        {
            return Err(HostLifetimeError::Denied);
        }
        if let Some(standing) = &admission.standing_operations {
            // Validate retained locators before any recovery/retirement can
            // use them. A store row cannot name another operator-owned root.
            let bundle = crate::installed_host::dynamic_members::callbacks::bundle(
                &self.key.realm,
                &self.key.owner,
                admission,
            )?;
            let scope = meerkat_mobkit::memory::MemoryScope::Identity {
                realm: self.member_realm()?.to_string(),
                identity: meerkat_mobkit::member_comms_id::durable_identity_for_roster_member(
                    &admission.instance.member.member.member,
                )
                .to_string(),
            }
            .operation_scope_ref();
            if standing.peer_message_parent.as_str() != format!("{bundle}/peer-message")
                || standing.recorder_write_parent.as_str() != format!("{bundle}/recorder-write")
                || standing.recorder_scope != scope
                || !admission.class.purpose.is_ordinary()
                || admission.workflow.is_some()
                || admission.route.is_none()
                || standing.peer_message_parent == standing.recorder_write_parent
                || standing.issued.as_ref().is_some_and(|issued| {
                    issued.peer_message.grant_id != standing.peer_message_parent
                        || issued.recorder_write.grant_id != standing.recorder_write_parent
                })
                || matches!(admission.phase, MemberClassAdmissionPhase::Active { .. })
                    && standing.issued.is_none()
            {
                return Err(invalid());
            }
        }
        if let Some(workflow) = &admission.workflow
            && (workflow.parent == workflow.original
                || admission
                    .route
                    .as_ref()
                    .is_some_and(|route| route.id != workflow.original.route))
        {
            return Err(invalid());
        }
        if let Some(resource) = &admission.workflow_resource
            && (admission.workflow.is_none()
                || admission.phase == MemberClassAdmissionPhase::Reserved
                || !resource.path.is_absolute()
                || resource.sha256.len() != 64
                || !resource
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(invalid());
        }
        use meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin;
        match (&admission.phase, &admission.route) {
            (MemberClassAdmissionPhase::Reserved, None)
            | (MemberClassAdmissionPhase::Prepared { .. }, _)
            | (MemberClassAdmissionPhase::Retired, _) => {}
            (MemberClassAdmissionPhase::Active { .. }, Some(route))
                if matches!(&route.origin, CompiledNativeSourceOrigin::LocalConsole { member, .. }
                    if member == &admission.instance.member.member) => {}
            _ => return Err(invalid()),
        }
        if admission.destination.as_ref().is_some_and(|destination| {
            destination.context_generation < admission.instance.member.context_generation
        }) {
            return Err(invalid());
        }
        if matches!(
            admission.phase,
            MemberClassAdmissionPhase::Prepared { .. } | MemberClassAdmissionPhase::Active { .. }
        ) && admission.destination.is_none()
        {
            return Err(invalid());
        }
        if let MemberClassAdmissionPhase::Prepared {
            context_generation, ..
        }
        | MemberClassAdmissionPhase::Active {
            context_generation, ..
        } = &admission.phase
            && (*context_generation < admission.instance.member.context_generation
                || admission
                    .destination
                    .as_ref()
                    .is_none_or(|selected| *context_generation > selected.context_generation))
        {
            return Err(invalid());
        }
        if let Some(route) = &admission.route
            && !matches!(&route.origin, CompiledNativeSourceOrigin::LocalConsole { member, .. }
            if member == &admission.instance.member.member)
        {
            return Err(invalid());
        }
        match &admission.phase {
            MemberClassAdmissionPhase::Reserved if admission.native_surface.is_some() => {
                return Err(invalid());
            }
            MemberClassAdmissionPhase::Prepared { .. }
            | MemberClassAdmissionPhase::Active { .. }
                if admission.native_surface.is_none() || admission.installed_surface.is_none() =>
            {
                return Err(invalid());
            }
            _ => {}
        }
        if let Some(surface) = &admission.native_surface
            && (admission
                .installed_surface
                .iter()
                .flatten()
                .any(|tool| !surface.contains(tool))
                || surface
                    .iter()
                    .enumerate()
                    .any(|(i, tool)| surface[..i].iter().any(|prior| prior.name == tool.name)))
        {
            return Err(invalid());
        }
        admission
            .class
            .materialize(&self.member_realm()?, &admission.instance)?;
        if admission.installed_surface.as_ref().is_some_and(|surface| {
            surface
                .iter()
                .enumerate()
                .any(|(i, tool)| surface[..i].iter().any(|prior| prior.name == tool.name))
                || admission.class.callbacks.iter().any(|callback| {
                    !surface
                        .iter()
                        .any(|tool| tool.name.as_str() == callback.name)
                })
        }) {
            return Err(invalid());
        }
        Ok(())
    }
    pub(super) fn validate_member_slot(&self, slot: &Slot) -> Result<(), HostLifetimeError> {
        if slot.pending.is_some() && slot.member_pending.is_some() {
            return Err(invalid());
        }
        for (i, admission) in slot.admissions.iter().enumerate() {
            self.validate_admission(admission)?;
            if slot.admissions[..i].iter().any(|prior| {
                prior.id == admission.id
                    || prior.instance.member.member.member
                        == admission.instance.member.member.member
            }) {
                return Err(invalid());
            }
        }
        for (decision, pending) in slot
            .member_decision
            .iter()
            .map(|d| (d, false))
            .chain(slot.member_pending.iter().map(|d| (d, true)))
        {
            self.validate_admission(&decision.admission)?;
            if decision.candidate != self.member_candidate(&decision.prior, &decision.admission)?
                || (pending && decision.prior != slot.current)
                || (!pending && decision.candidate != slot.current)
                || !slot.admissions.iter().any(|row| {
                    row.instance == decision.admission.instance
                        && row.class == decision.admission.class
                        && ((row.id == decision.admission.id
                            && row.installed_surface == decision.admission.installed_surface
                            && row.native_surface == decision.admission.native_surface)
                            // A new conditional admission can replace the
                            // retired instance while its previous narrowing
                            // decision still proves the current policy bytes.
                            || (!pending && row.id != decision.admission.id
                                && decision.admission.phase == MemberClassAdmissionPhase::Retired
                                && decision.admission.destination.as_ref().is_none_or(|prior|
                                    row.destination.as_ref().is_some_and(|current|
                                        current.context_generation >= prior.context_generation))))
                })
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub(super) fn validate_service_preservation(
        &self,
        slot: &Slot,
        decision: &ActiveMemberToolPolicyDecision,
    ) -> Result<(), HostLifetimeError> {
        let approved = CompiledApplicationToolPolicy::parse_canonical_json(
            decision.proposal.candidate_canonical_json.as_bytes(),
        )
        .map_err(|_| invalid())?;
        let current = CompiledApplicationToolPolicy::parse_canonical_json(slot.current.as_bytes())
            .map_err(|_| invalid())?;
        if approved.provider_id != current.provider_id
            || approved.policy_id != current.policy_id
            || current.revision.0 < approved.revision.0
        {
            return Err(invalid());
        }
        let bootstrap =
            CompiledApplicationToolPolicy::parse_canonical_json(self.bootstrap.as_bytes())
                .map_err(|_| invalid())?;
        for initial in &bootstrap.members {
            if !approved
                .members
                .iter()
                .any(|row| row.member_identity == initial.member_identity)
                || !current
                    .members
                    .iter()
                    .any(|row| row.member_identity == initial.member_identity)
            {
                return Err(invalid());
            }
        }
        for row in &approved.members {
            match current
                .members
                .iter()
                .find(|now| now.member_identity == row.member_identity)
            {
                Some(now) if now == row => {}
                None => self.validate_absent_service_member(slot, row)?,
                _ => return Err(invalid()),
            }
        }
        for row in &current.members {
            if approved
                .members
                .iter()
                .any(|prior| prior.member_identity == row.member_identity)
            {
                continue;
            }
            let admission = slot
                .admissions
                .iter()
                .find(|a| a.instance.member.member.member == row.member_identity)
                .ok_or_else(invalid)?;
            if admission.phase == MemberClassAdmissionPhase::Retired
                || admission
                    .class
                    .materialize(&self.member_realm()?, &admission.instance)?
                    != *row
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    fn validate_absent_service_member(
        &self,
        slot: &Slot,
        row: &meerkat_core::CompiledMemberToolGrants,
    ) -> Result<(), HostLifetimeError> {
        let mut matches = slot
            .admissions
            .iter()
            .filter(|admission| admission.instance.member.member.member == row.member_identity);
        let admission = matches.next().ok_or_else(invalid)?;
        if matches.next().is_some() {
            return Err(invalid());
        }
        self.validate_admission(admission)?;
        if admission
            .class
            .materialize(&self.member_realm()?, &admission.instance)?
            != *row
        {
            return Err(invalid());
        }
        match &admission.phase {
            // This only preserves the old service approval while the current
            // canonical policy omits the class row. It grants no member authority and
            // does not depend on which unrelated member changed most recently.
            MemberClassAdmissionPhase::Reserved | MemberClassAdmissionPhase::Retired => Ok(()),
            MemberClassAdmissionPhase::Prepared { .. } => {
                let pending = slot.member_pending.as_ref().ok_or_else(invalid)?;
                if pending.admission != *admission
                    || pending.prior != slot.current
                    || pending.candidate != self.member_candidate(&pending.prior, admission)?
                {
                    return Err(invalid());
                }
                let candidate = CompiledApplicationToolPolicy::parse_canonical_json(
                    pending.candidate.as_bytes(),
                )
                .map_err(|_| invalid())?;
                if !candidate.members.contains(row) {
                    return Err(invalid());
                }
                Ok(())
            }
            MemberClassAdmissionPhase::Active { .. } => Err(invalid()),
        }
    }
    fn reserve_member(
        &self,
        admission: &MemberClassAdmission,
        canonical: &str,
    ) -> Result<Arc<dyn ApplicationServicePolicyReservation>, HostLifetimeError> {
        let realm = self.member_realm()?;
        let decision = self
            .participant
            .as_ref()
            .ok_or_else(unavailable)?
            .reserve_member_policy(&ApplicationMemberPolicyRequest {
                owner: &self.application,
                realm: &realm,
                deployment: &self.key.owner,
                admission,
                canonical_policy: canonical.as_bytes(),
            })?;
        let reservation: Arc<dyn ApplicationServicePolicyReservation> = match decision {
            ApplicationToolPolicyRecoveryDecision::RequiresHuman => {
                return Err(HostLifetimeError::Denied);
            }
            ApplicationToolPolicyRecoveryDecision::Permit(value) => Arc::from(value),
        };
        if reservation.owner() != &self.application {
            return Err(HostLifetimeError::WrongOwner);
        }
        reservation.check_current()?;
        Ok(reservation)
    }
    // A revoked instance cannot make another instance unavailable. Missing or
    // revoked reservations stay absent and fail that instance's exact lookup.
    pub(super) fn member_reservations(
        &self,
        slot: &Slot,
    ) -> Result<MemberReservations, HostLifetimeError> {
        Ok(slot
            .admissions
            .iter()
            .filter(|a| a.phase != MemberClassAdmissionPhase::Retired)
            .map(|admission| {
                (
                    admission.id.to_string(),
                    self.reserve_member(admission, &slot.current),
                )
            })
            .collect())
    }
    fn member_candidate(
        &self,
        bytes: &str,
        admission: &MemberClassAdmission,
    ) -> Result<String, HostLifetimeError> {
        let current = CompiledApplicationToolPolicy::parse_canonical_json(bytes.as_bytes())
            .map_err(|_| invalid())?;
        let row = admission
            .class
            .materialize(&self.member_realm()?, &admission.instance)?;
        let exists = current
            .members
            .iter()
            .any(|prior| prior.member_identity == row.member_identity);
        let retiring = admission.phase == MemberClassAdmissionPhase::Retired;
        if exists && !retiring {
            return Err(HostLifetimeError::Denied);
        }
        let mut members = current.members.clone();
        if retiring {
            members.retain(|prior| prior.member_identity != row.member_identity);
        } else {
            members.push(row);
        }
        // A pending addition may already have been accepted before its durable
        // promotion failed. An absent-row retirement skips that possible
        // revision without replaying or reauthorizing the expired addition.
        let advance = if retiring && !exists { 2 } else { 1 };
        members.sort_by(|a, b| a.member_identity.cmp(&b.member_identity));
        let source = canonical_json(&(&self.key, admission, bytes)).map_err(|_| invalid())?;
        let compiled = CompiledApplicationToolPolicy::new(
            current.provider_id,
            current.policy_id,
            PolicyRevision(
                current
                    .revision
                    .0
                    .checked_add(advance)
                    .ok_or_else(invalid)?,
            ),
            CompiledPolicySourceProvenance {
                source_id: "toolkit-application-member-decision".into(),
                source_digest: PolicyDigest::from_canonical_bytes(&source),
            },
            members,
        )
        .map_err(|_| invalid())?;
        String::from_utf8(compiled.canonical_json().map_err(|_| invalid())?).map_err(|_| invalid())
    }
    pub(crate) async fn admit_member(
        self: &Arc<Self>,
        request: MemberClassAdmissionRequest,
    ) -> Result<AdmittedMemberClass, HostLifetimeError> {
        request.validate(&self.member_realm()?, &self.classes)?;
        self.admit_member_selected(
            request.id,
            request.instance,
            Some(request.tools),
            None,
            None,
        )
        .await
    }
    pub(crate) async fn readmit_member(
        self: &Arc<Self>,
        expected: MemberClassCheckpoint,
        request: MemberClassAdmissionRequest,
    ) -> Result<AdmittedMemberClass, HostLifetimeError> {
        request.validate(&self.member_realm()?, &self.classes)?;
        self.admit_member_selected(
            request.id,
            request.instance,
            Some(request.tools),
            Some(expected),
            None,
        )
        .await
    }
    /// Observe the exact member and predecessor from one canonical read. This
    /// does not recover, publish or refresh the ordinary enforcement snapshot.
    pub(crate) async fn observe_member(
        &self,
        member: &MobMemberBinding,
    ) -> Result<MemberClassObservation, HostLifetimeError> {
        let _exclusion = self.exclusion.lock().await;
        let (stored, slot) = self.read().await?;
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        let admission = slot
            .admissions
            .iter()
            .find(|row| &row.instance.member.member == member)
            .ok_or(HostLifetimeError::Denied)?
            .clone();
        let authorization = if admission.phase == MemberClassAdmissionPhase::Retired {
            Err(HostLifetimeError::Denied)
        } else {
            (|| {
                // The complete-policy reservation is selected from this same
                // slot, not combined with a separately cached admission read.
                let policy = slot
                    .decision
                    .as_ref()
                    .map(|decision| {
                        self.recovery_reservation(
                            decision,
                            decision.proposal.candidate_canonical_json.as_bytes(),
                            &slot,
                        )
                    })
                    .transpose()?;
                let member = self.reserve_member(&admission, &slot.current)?;
                if let Some(policy) = policy {
                    policy.check_current()?;
                }
                member.check_current()
            })()
        };
        if let Err(error @ (HostLifetimeError::InvalidSelection | HostLifetimeError::WrongOwner)) =
            authorization
        {
            return Err(error);
        }
        Ok(MemberClassObservation {
            checkpoint: MemberClassCheckpoint {
                admission: admission.id.clone(),
                revision: stored.revision,
            },
            admission,
            authorization,
        })
    }
    pub(crate) async fn member_checkpoint(
        &self,
        member: &MobMemberBinding,
    ) -> Result<MemberClassCheckpoint, HostLifetimeError> {
        let (stored, slot) = self.read().await?;
        let row = slot
            .admissions
            .iter()
            .find(|a| &a.instance.member.member == member)
            .ok_or(HostLifetimeError::Denied)?;
        Ok(MemberClassCheckpoint {
            admission: row.id.clone(),
            revision: stored.revision,
        })
    }
    pub(crate) async fn admit_member_declaration(
        self: &Arc<Self>,
        id: toolkit_package::Id,
        instance: meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance,
    ) -> Result<AdmittedMemberClass, HostLifetimeError> {
        self.admit_member_selected(id, instance, None, None, None)
            .await
    }
    pub(crate) async fn readmit_member_declaration(
        self: &Arc<Self>,
        expected: MemberClassCheckpoint,
        id: toolkit_package::Id,
        instance: meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance,
    ) -> Result<AdmittedMemberClass, HostLifetimeError> {
        self.admit_member_selected(id, instance, None, Some(expected), None)
            .await
    }
    pub(crate) async fn admit_workflow_member_declaration(
        self: &Arc<Self>,
        id: toolkit_package::Id,
        instance: meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance,
        workflow: MemberClassWorkflowBinding,
    ) -> Result<AdmittedMemberClass, HostLifetimeError> {
        self.admit_member_selected(id, instance, None, None, Some(workflow))
            .await
    }
    async fn admit_member_selected(
        self: &Arc<Self>,
        id: toolkit_package::Id,
        instance: meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance,
        tools: Option<PreparedMemberClassTools>,
        expected: Option<MemberClassCheckpoint>,
        workflow: Option<MemberClassWorkflowBinding>,
    ) -> Result<AdmittedMemberClass, HostLifetimeError> {
        let class = self
            .classes
            .iter()
            .find(|class| class.id == instance.class)
            .ok_or(HostLifetimeError::Denied)?
            .clone();
        if instance.member.setup_capability.is_some()
            || class.purpose.is_ordinary() != workflow.is_none()
            || workflow
                .as_ref()
                .is_some_and(|binding| binding.parent == binding.original)
            || tools.as_ref().is_some_and(|tools| tools.class() != &class)
        {
            return Err(HostLifetimeError::Denied);
        }
        class.materialize(&self.member_realm()?, &instance)?;
        self.recover().await?;
        let _exclusion = self.exclusion.lock().await;
        let (stored, mut slot) = self.read().await?;
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        if expected
            .as_ref()
            .is_some_and(|prior| prior.revision != stored.revision)
        {
            return Err(HostLifetimeError::Denied);
        }
        let admission = MemberClassAdmission {
            id,
            class,
            instance,
            workflow,
            workflow_resource: None,
            destination: None,
            installed_surface: tools.as_ref().map(|tools| tools.identities().to_vec()),
            native_surface: None,
            route: None,
            standing_operations: None,
            phase: MemberClassAdmissionPhase::Reserved,
        };
        if let Some(prior) = slot.admissions.iter().find(|a| a.id == admission.id) {
            if expected.is_some()
                || prior.class != admission.class
                || prior.workflow != admission.workflow
                || prior.instance != admission.instance
                || admission
                    .installed_surface
                    .as_ref()
                    .is_some_and(|requested| {
                        prior
                            .installed_surface
                            .as_ref()
                            .is_some_and(|prior| prior != requested)
                    })
                || prior.phase == MemberClassAdmissionPhase::Retired
            {
                return Err(HostLifetimeError::Denied);
            }
            self.reserve_member(prior, &slot.current)?.check_current()?;
            return Ok(AdmittedMemberClass {
                owner: self.clone(),
                admission: prior.clone(),
                tools,
                add_token: None,
            });
        }
        let prior_member = slot.admissions.iter().position(|a| {
            a.instance.member.member.member == admission.instance.member.member.member
        });
        let mut admission = admission;
        if let Some(index) = prior_member {
            let prior = &slot.admissions[index];
            // An explicit new request can re-admit this same logical member.
            // Same-id replay above never resurrects a retired decision, and a
            // different class/destination cannot inherit its identity/history.
            if expected
                .as_ref()
                .is_none_or(|expected| expected.admission != prior.id)
                || prior.phase != MemberClassAdmissionPhase::Retired
                || prior.class != admission.class
                || prior.instance != admission.instance
                || prior.workflow != admission.workflow
                || prior
                    .installed_surface
                    .as_ref()
                    .zip(admission.installed_surface.as_ref())
                    .is_some_and(|(before, after)| before != after)
            {
                return Err(HostLifetimeError::Denied);
            }
            admission.destination = prior.destination.clone();
        } else if expected.is_some() {
            return Err(HostLifetimeError::Denied);
        }
        let current = CompiledApplicationToolPolicy::parse_canonical_json(slot.current.as_bytes())
            .map_err(|_| invalid())?;
        if current
            .members
            .iter()
            .any(|a| a.member_identity == admission.instance.member.member.member)
        {
            return Err(HostLifetimeError::Denied);
        }
        let reservation = self.reserve_member(&admission, &slot.current)?;
        if let Some(index) = prior_member {
            // The previous decision still proves the canonical policy revision.
            // Keep its exact retired row separately in that decision; validation
            // below permits only this explicit, equivalent reserved successor.
            slot.admissions[index] = admission.clone();
        } else {
            slot.admissions.push(admission.clone());
        }
        self.change(&stored, &slot, "member-admitted").await?;
        reservation.check_current()?;
        self.activate_current(&slot)?;
        Ok(AdmittedMemberClass {
            owner: self.clone(),
            admission,
            tools,
            add_token: Some(MemberClassAddToken::new()),
        })
    }

    /// Claim is synchronous so the host can transfer ownership without a
    /// cancellation point between consumption and its existing task spawn.
    pub(crate) fn claim_member_add(
        &self,
        expected: &MemberClassAdmission,
        token: &MemberClassAddToken,
    ) -> Result<(), HostLifetimeError> {
        let _exclusion = self.exclusion.try_lock().map_err(|_| unavailable())?;
        let current = self
            .observed_admission(&expected.instance.member.member)?
            .ok_or(HostLifetimeError::Denied)?;
        if current != *expected || current.phase != MemberClassAdmissionPhase::Reserved {
            return Err(HostLifetimeError::Denied);
        }
        token.consume()
    }

    /// Retain a checked construction observation on the existing admission.
    /// This records callback identity only and never changes tool grants.
    pub(crate) async fn bind_member_surface(
        &self,
        expected: &MemberClassAdmission,
        identities: &[meerkat_core::ToolIdentity],
    ) -> Result<(), HostLifetimeError> {
        let observed = self
            .observed_admission(&expected.instance.member.member)?
            .ok_or(HostLifetimeError::Denied)?;
        if &observed != expected
            || identities
                .iter()
                .enumerate()
                .any(|(i, tool)| identities[..i].iter().any(|prior| prior.name == tool.name))
            || expected.class.callbacks.iter().any(|callback| {
                !identities
                    .iter()
                    .any(|tool| tool.name.as_str() == callback.name)
            })
        {
            return Err(HostLifetimeError::Denied);
        }
        if let Some(prior) = &observed.installed_surface {
            return if prior == identities {
                Ok(())
            } else {
                Err(invalid())
            };
        }
        if observed.phase != MemberClassAdmissionPhase::Reserved {
            return Err(HostLifetimeError::Denied);
        }
        let _exclusion = self.exclusion.lock().await;
        let (stored, mut slot) = self.read().await?;
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        let selected = slot
            .admissions
            .iter_mut()
            .find(|a| a.id == expected.id)
            .ok_or(HostLifetimeError::Denied)?;
        if selected != expected {
            return Err(HostLifetimeError::Denied);
        }
        selected.installed_surface = Some(identities.to_vec());
        let selected = selected.clone();
        let reservation = self.reserve_member(&selected, &slot.current)?;
        self.change(&stored, &slot, "member-callbacks-observed")
            .await?;
        *self.observed_current.lock().map_err(|_| unavailable())? = None;
        reservation.check_current()?;
        self.activate_current(&slot)?;
        Ok(())
    }
    /// Bind immutable host evidence through the existing exact admission CAS.
    /// The artifact is provenance only. Its physical/native/approval owners
    /// remain responsible for every later use or reconstruction decision.
    pub(crate) async fn retain_workflow_resource(
        &self,
        expected: &MemberClassAdmission,
        checkpoint: &MemberClassCheckpoint,
        resource: meerkat_toolkit::host_lifetime::control::SelectedHostArtifact,
    ) -> Result<MemberClassAdmission, HostLifetimeError> {
        let _exclusion = self.exclusion.lock().await;
        self.check_current()?;
        let observed = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        let (stored, mut slot) = self.read().await?;
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        if checkpoint.admission != expected.id
            || checkpoint.revision != stored.revision
            || observed.authority.canonical != slot.current
            || observed.admissions != slot.admissions
        {
            return Err(HostLifetimeError::Denied);
        }
        let selected = slot
            .admissions
            .iter_mut()
            .find(|row| row.id == expected.id)
            .ok_or(HostLifetimeError::Denied)?;
        if selected != expected
            || selected.workflow.is_none()
            || selected.route.is_none()
            || selected.workflow_resource.as_ref() == Some(&resource)
            || !matches!(&selected.phase,
                MemberClassAdmissionPhase::Prepared { context, context_generation }
                | MemberClassAdmissionPhase::Active { context, context_generation }
                if selected.destination.as_ref().is_some_and(|destination|
                    destination.context == *context && destination.context_generation == *context_generation))
        {
            return Err(HostLifetimeError::Denied);
        }
        selected.workflow_resource = Some(resource);
        self.validate_admission(selected)?;
        let selected = selected.clone();
        let member = self.reserve_member(&selected, &slot.current)?;
        if let Some(policy) = &observed.authority.reservation {
            policy.check_current()?;
        }
        member.check_current()?;
        if !self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &observed))
        {
            return Err(unavailable());
        }
        // Keep the original MemberPolicyDecision unchanged: its admission
        // is part of the accepted native policy's provenance hash. Updating
        // resource metadata must not reaccept or revise native policy bytes.
        self.change(&stored, &slot, "workflow-resource-selected")
            .await?;
        // Once the write commits, the prior cached admission cannot remain
        // available if current authorization has disappeared meanwhile.
        {
            let mut current = self.observed_current.lock().map_err(|_| unavailable())?;
            if !current
                .as_ref()
                .is_some_and(|now| Arc::ptr_eq(now, &observed))
            {
                return Err(unavailable());
            }
            *current = None;
        }
        if let Some(policy) = &observed.authority.reservation {
            policy.check_current()?;
        }
        member.check_current()?;
        let mut members = observed.member_reservations.clone();
        let reserved = members
            .iter_mut()
            .find(|(id, _)| id == selected.id.as_str())
            .ok_or_else(unavailable)?;
        reserved.1 = Ok(member);
        *self.observed_current.lock().map_err(|_| unavailable())? =
            Some(Arc::new(ObservedPolicy {
                authority: observed.authority.clone(),
                admissions: slot.admissions,
                member_reservations: members,
            }));
        Ok(selected)
    }

    // Caller holds canonical exclusion and has committed only this member's
    // registration metadata. Keep unrelated observations available, but never
    // expose the changed destination with its prior reservation.
    fn observe_registration_metadata(
        &self,
        slot: &Slot,
        authority: &Arc<ObservedPolicyAuthority>,
        id: &toolkit_package::Id,
        reservation: Result<Arc<dyn ApplicationServicePolicyReservation>, HostLifetimeError>,
    ) -> Result<(), HostLifetimeError> {
        self.check_current()?;
        let mut current = self.observed_current.lock().map_err(|_| unavailable())?;
        let observed = current.as_ref().ok_or_else(unavailable)?;
        if !Arc::ptr_eq(&observed.authority, authority)
            || slot.pending.is_some()
            || slot.member_pending.is_some()
            || observed.authority.canonical != slot.current
            || observed.admissions.len() != slot.admissions.len()
        {
            return Err(unavailable());
        }
        for (prior, selected) in observed.admissions.iter().zip(&slot.admissions) {
            let mut expected = prior.clone();
            if &prior.id == id {
                expected.destination = selected.destination.clone();
                match (&mut expected.phase, &selected.phase) {
                    (
                        MemberClassAdmissionPhase::Prepared {
                            context,
                            context_generation,
                        },
                        MemberClassAdmissionPhase::Prepared {
                            context: next,
                            context_generation: generation,
                        },
                    )
                    | (
                        MemberClassAdmissionPhase::Active {
                            context,
                            context_generation,
                        },
                        MemberClassAdmissionPhase::Active {
                            context: next,
                            context_generation: generation,
                        },
                    ) => {
                        *context = next.clone();
                        *context_generation = *generation;
                    }
                    _ => {}
                }
            }
            if expected != *selected {
                return Err(HostLifetimeError::Denied);
            }
        }
        let mut members = observed.member_reservations.clone();
        let selected = members
            .iter_mut()
            .find(|(member, _)| member == id.as_str())
            .ok_or(HostLifetimeError::Denied)?;
        selected.1 = reservation;
        *current = Some(Arc::new(ObservedPolicy {
            authority: observed.authority.clone(),
            admissions: slot.admissions.clone(),
            member_reservations: members,
        }));
        Ok(())
    }

    pub(crate) fn observed_admission(
        &self,
        member: &MobMemberBinding,
    ) -> Result<Option<MemberClassAdmission>, HostLifetimeError> {
        self.check_current()?;
        let observed = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        let Some(admission) = observed
            .admissions
            .iter()
            .find(|a| a.instance.member.member.member == member.member)
        else {
            return Ok(None);
        };
        if &admission.instance.member.member != member
            || admission.phase == MemberClassAdmissionPhase::Retired
        {
            return Err(HostLifetimeError::Denied);
        }
        let reservation = observed
            .member_reservations
            .iter()
            .find(|(id, _)| id == admission.id.as_str())
            .ok_or(HostLifetimeError::Denied)?;
        let reservation = reservation.1.as_ref().map_err(|error| *error)?;
        reservation.check_current()?;
        if !self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .is_some_and(|current| {
                Arc::ptr_eq(&current.authority, &observed.authority)
                    && current.admissions.iter().find(|row| row.id == admission.id)
                        == Some(admission)
                    && current
                        .member_reservations
                        .iter()
                        .find(|(id, _)| id == admission.id.as_str())
                        .and_then(|(_, value)| value.as_ref().ok())
                        .is_some_and(|value| Arc::ptr_eq(value, reservation))
            })
        {
            return Err(unavailable());
        }
        Ok(Some(admission.clone()))
    }
    pub(crate) fn member_build_admission(
        &self,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
    ) -> Result<Option<MemberClassAdmission>, HostLifetimeError> {
        self.check_current()?;
        let native =
            meerkat_mobkit::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let observed = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        let member = observed
            .admissions
            .iter()
            .find(|row| row.instance.member.member.member == native.as_str())
            .map(|row| row.instance.member.member.clone());
        match member {
            Some(member) => self.observed_admission(&member),
            None => Ok(None),
        }
    }
    pub(crate) fn verify_member_surface_binding(
        &self,
        selection: &meerkat_core::ToolSurfaceSelection,
    ) -> Result<(), HostLifetimeError> {
        self.check_current()?;
        let observed = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        let policy = CompiledApplicationToolPolicy::parse_canonical_json(
            observed.authority.canonical.as_bytes(),
        )
        .map_err(|_| invalid())?;
        if selection.binding
            != (meerkat_core::ApplicationToolPolicyBinding::Provider {
                provider_id: policy.provider_id,
                policy_id: policy.policy_id,
            })
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }
    pub(crate) fn member_capacity(&self) -> Result<usize, HostLifetimeError> {
        self.check_current()?;
        let observed = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        Ok(observed
            .admissions
            .iter()
            .filter(|a| a.phase != MemberClassAdmissionPhase::Retired)
            .count())
    }
    pub(crate) async fn select_member_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<MemberDestinationSelection>, HostLifetimeError> {
        self.select_registration(
            identity.realm_id(),
            identity.member(),
            identity.registration(),
        )
        .await
    }

    // Called only with the native hook's exact witness. Keeping the primitive
    // explicit also lets owner tests use real machine-minted registrations.
    async fn select_registration(
        &self,
        realm: &RealmId,
        member: &MobMemberBinding,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Result<Option<MemberDestinationSelection>, HostLifetimeError> {
        if realm != &self.member_realm()? || registration.has_ended() {
            return Err(HostLifetimeError::Denied);
        }
        // Opening the installed owner already reconciles its canonical policy.
        // Repeating recovery for each native registration temporarily clears
        // that shared observation and can refuse another concurrent restore.
        // A pending or revoked owner still refuses admission; registration is
        // not the owner of policy recovery.
        self.check_current()?;
        let Some(observed) = self.observed_admission(member)? else {
            return Ok(None);
        };
        let _exclusion = self.exclusion.lock().await;
        let (stored, mut slot) = self.read().await?;
        let authority = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .ok_or_else(unavailable)?
            .authority
            .clone();
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        let selected = slot
            .admissions
            .iter_mut()
            .find(|row| row.id == observed.id)
            .ok_or(HostLifetimeError::Denied)?;
        if selected != &observed {
            return Err(HostLifetimeError::Denied);
        }
        let same_registration = match self
            .member_registrations
            .lock()
            .map_err(|_| unavailable())?
            .get(&member.member)
        {
            Some(prior) => {
                if prior.admission != selected.id
                    || selected.destination.as_ref() != Some(&prior.destination)
                {
                    return Err(unavailable());
                }
                prior.registration == *registration
            }
            None => false,
        };
        let context = meerkat_authorization_contracts::evidence::EvidenceId::new(
            registration.session_id().to_string(),
        )
        .map_err(|_| invalid())?;
        let generation = match &selected.destination {
            Some(prior) if same_registration || prior.context != context => prior
                .context_generation
                .checked_add(1)
                .ok_or_else(invalid)?,
            Some(prior) => prior.context_generation,
            None => selected.instance.member.context_generation,
        };
        let destination = MemberClassDestination {
            context,
            context_generation: generation,
        };
        selected.destination = Some(destination.clone());
        let selected = selected.clone();
        let reservation = self.reserve_member(&selected, &slot.current)?;
        // A replacement registration binds the same logical destination; it
        // does not rewrite retained work's target. A retry on this exact native
        // witness advances because Core can retain its failed revival binding.
        // A cold cache is not proof that Core is unbound: its binding gate must
        // still accept this selection. If it refuses, the recorded attempt
        // below makes an explicit same-witness repair advance next time.
        if let Some(prior) = self
            .member_registrations
            .lock()
            .map_err(|_| unavailable())?
            .get_mut(&member.member)
        {
            prior.verified = None;
        }
        self.change(&stored, &slot, "member-destination-selected")
            .await?;
        self.observe_registration_metadata(&slot, &authority, &selected.id, Err(unavailable()))?;
        reservation.check_current()?;
        if registration.has_ended() {
            return Err(HostLifetimeError::Denied);
        }
        self.observe_registration_metadata(&slot, &authority, &selected.id, Ok(reservation))?;
        self.member_registrations
            .lock()
            .map_err(|_| unavailable())?
            .insert(
                member.member.clone(),
                MemberRegistrationSelection {
                    admission: selected.id.clone(),
                    destination,
                    registration: registration.clone(),
                    verified: None,
                },
            );
        self.expected_registration(realm, member, registration)
    }

    pub(crate) fn member_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<MemberDestinationSelection>, HostLifetimeError> {
        self.expected_registration(
            identity.realm_id(),
            identity.member(),
            identity.registration(),
        )
    }

    fn expected_registration(
        &self,
        realm: &RealmId,
        member: &MobMemberBinding,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Result<Option<MemberDestinationSelection>, HostLifetimeError> {
        if realm != &self.member_realm()? || registration.has_ended() {
            return Err(HostLifetimeError::Denied);
        }
        let Some(admission) = self.observed_admission(member)? else {
            return Ok(None);
        };
        let destination = admission
            .destination
            .as_ref()
            .ok_or(HostLifetimeError::Denied)?;
        let registrations = self
            .member_registrations
            .lock()
            .map_err(|_| unavailable())?;
        let selected = registrations
            .get(&member.member)
            .ok_or(HostLifetimeError::Denied)?;
        if selected.admission != admission.id
            || selected.registration != *registration
            || selected.destination != *destination
            || destination.context.as_str() != registration.session_id().to_string()
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(Some(MemberDestinationSelection {
            logical_owner: admission.instance.member.logical_owner,
            context: destination.context.clone(),
            context_generation: destination.context_generation,
            audience: admission.instance.member.audience,
        }))
    }

    /// A recovered actor supplies its actual visible surface before publication.
    /// This carries an existing Active authorization to the newly selected
    /// context; it never activates a fresh held admission or broadens grants.
    pub(crate) async fn verify_member_registration(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<(), HostLifetimeError> {
        self.verify_registration(
            identity.realm_id(),
            identity.member(),
            identity.registration(),
        )
        .await
    }
    // Same owner boundary as select_registration: the actual hook supplies the
    // identity, and owner controls can use a real machine-minted witness.
    async fn verify_registration(
        &self,
        realm: &RealmId,
        member: &MobMemberBinding,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Result<(), HostLifetimeError> {
        let destination = self
            .expected_registration(realm, member, registration)?
            .ok_or(HostLifetimeError::Denied)?;
        let _exclusion = self.exclusion.lock().await;
        let (stored, mut slot) = self.read().await?;
        let authority = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .ok_or_else(unavailable)?
            .authority
            .clone();
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        let selected = slot
            .admissions
            .iter_mut()
            .find(|row| &row.instance.member.member == member)
            .ok_or(HostLifetimeError::Denied)?;
        let expected = MemberClassDestination {
            context: destination.context.clone(),
            context_generation: destination.context_generation,
        };
        if selected.destination.as_ref() != Some(&expected) {
            return Err(HostLifetimeError::Denied);
        }
        if let MemberClassAdmissionPhase::Active {
            context,
            context_generation,
        } = &mut selected.phase
            && (context != &destination.context
                || *context_generation != destination.context_generation)
        {
            *context = destination.context;
            *context_generation = destination.context_generation;
            let id = selected.id.clone();
            let reservation = self.reserve_member(selected, &slot.current)?;
            self.change(&stored, &slot, "member-registration-verified")
                .await?;
            self.observe_registration_metadata(&slot, &authority, &id, Err(unavailable()))?;
            reservation.check_current()?;
            if registration.has_ended() {
                return Err(HostLifetimeError::Denied);
            }
            self.observe_registration_metadata(&slot, &authority, &id, Ok(reservation))?;
        }
        self.mark_registration_verified(realm, member, registration)
    }
    fn mark_registration_verified(
        &self,
        realm: &RealmId,
        member: &MobMemberBinding,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Result<(), HostLifetimeError> {
        // Caller holds canonical exclusion after actual native verification.
        self.expected_registration(realm, member, registration)?
            .ok_or(HostLifetimeError::Denied)?;
        let mut registrations = self
            .member_registrations
            .lock()
            .map_err(|_| unavailable())?;
        let selected = registrations
            .get_mut(&member.member)
            .ok_or(HostLifetimeError::Denied)?;
        if selected.registration != *registration || registration.has_ended() {
            return Err(HostLifetimeError::Denied);
        }
        selected.verified = Some(registration.clone());
        Ok(())
    }
    pub(crate) async fn prepare_member(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &PreparedToolSurface,
    ) -> Result<bool, HostLifetimeError> {
        #[cfg(test)]
        let mut diagnostic = MemberPreparationDiagnostic::new();
        let destination = self.member_destination(identity)?;
        #[cfg(test)]
        {
            diagnostic.stage = "observed_admission";
        }
        let Some(mut admission) = self.observed_admission(identity.member())? else {
            #[cfg(test)]
            {
                diagnostic.completed = true;
            }
            return Ok(false);
        };
        #[cfg(test)]
        {
            diagnostic.stage = "destination_missing";
        }
        let destination = destination.ok_or(HostLifetimeError::Denied)?;
        #[cfg(test)]
        {
            diagnostic.stage = "native_surface_invalid";
        }
        let native_surface = native_surface_identities(surface)?;
        #[cfg(test)]
        {
            diagnostic.stage = "installed_surface_missing_or_not_contained";
            diagnostic.surfaces(
                admission.installed_surface.as_deref(),
                Some(&native_surface),
            );
        }
        if admission
            .installed_surface
            .as_ref()
            .ok_or(HostLifetimeError::InvalidSelection)?
            .iter()
            .any(|tool| !native_surface.contains(tool))
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if let Some(prior) = &admission.native_surface
            && prior != &native_surface
        {
            #[cfg(test)]
            {
                diagnostic.stage = "prior_native_surface_changed";
                diagnostic.surfaces(Some(prior), Some(&native_surface));
            }
            return Err(HostLifetimeError::InvalidSelection);
        }
        #[cfg(test)]
        {
            diagnostic.stage = "exclusion_wait";
            diagnostic.expected = None;
            diagnostic.actual = None;
        }
        let _exclusion = self.exclusion.lock().await;
        #[cfg(test)]
        {
            diagnostic.stage = "canonical_read";
        }
        let (stored, mut slot) = self.read().await?;
        #[cfg(test)]
        {
            diagnostic.stage = "current_authority";
        }
        let authority = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .ok_or_else(unavailable)?
            .authority
            .clone();
        #[cfg(test)]
        {
            diagnostic.stage = "canonical_pending";
        }
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        #[cfg(test)]
        {
            diagnostic.stage = "canonical_admission_missing";
        }
        let selected = slot
            .admissions
            .iter_mut()
            .find(|a| a.id == admission.id)
            .ok_or_else(unavailable)?;
        #[cfg(test)]
        {
            diagnostic.stage = "canonical_admission_changed";
        }
        if selected != &admission {
            return Err(HostLifetimeError::Denied);
        }
        if !matches!(admission.phase, MemberClassAdmissionPhase::Reserved) {
            if let MemberClassAdmissionPhase::Prepared {
                context,
                context_generation,
            } = &mut selected.phase
                && (context != &destination.context
                    || *context_generation != destination.context_generation)
            {
                *context = destination.context;
                *context_generation = destination.context_generation;
                let id = selected.id.clone();
                #[cfg(test)]
                {
                    diagnostic.stage = "prepared_reselection_reservation";
                }
                let reservation = self.reserve_member(selected, &slot.current)?;
                #[cfg(test)]
                {
                    diagnostic.stage = "prepared_reselection_change";
                }
                self.change(&stored, &slot, "member-preparation-reselected")
                    .await?;
                #[cfg(test)]
                {
                    diagnostic.stage = "prepared_reselection_unavailable_observation";
                }
                self.observe_registration_metadata(&slot, &authority, &id, Err(unavailable()))?;
                #[cfg(test)]
                {
                    diagnostic.stage = "prepared_reselection_current";
                }
                reservation.check_current()?;
                #[cfg(test)]
                {
                    diagnostic.stage = "prepared_reselection_registration_ended";
                }
                if identity.registration().has_ended() {
                    return Err(HostLifetimeError::Denied);
                }
                #[cfg(test)]
                {
                    diagnostic.stage = "prepared_reselection_observation";
                }
                self.observe_registration_metadata(&slot, &authority, &id, Ok(reservation))?;
            }
            #[cfg(test)]
            {
                diagnostic.completed = true;
            }
            return Ok(true);
        }
        // This exact observation comes only from the native registration gate.
        // The policy remains the packaged grants; observing tools grants none.
        admission.native_surface = Some(native_surface);
        admission.phase = MemberClassAdmissionPhase::Prepared {
            context: destination.context,
            context_generation: destination.context_generation,
        };
        #[cfg(test)]
        {
            diagnostic.stage = "reserved_candidate";
        }
        let candidate = self.member_candidate(&slot.current, &admission)?;
        #[cfg(test)]
        {
            diagnostic.stage = "reserved_reservation_current";
        }
        self.reserve_member(&admission, &candidate)?
            .check_current()?;
        #[cfg(test)]
        {
            diagnostic.stage = "reserved_admission_missing";
        }
        *slot
            .admissions
            .iter_mut()
            .find(|a| a.id == admission.id)
            .ok_or_else(invalid)? = admission.clone();
        slot.member_pending = Some(MemberPolicyDecision {
            admission,
            prior: slot.current.clone(),
            candidate,
        });
        #[cfg(test)]
        {
            diagnostic.stage = "reserved_pending_change";
        }
        let stored = self.change(&stored, &slot, "member-policy-pending").await?;
        #[cfg(test)]
        {
            diagnostic.stage = "reserved_clear_observation";
        }
        *self.observed_current.lock().map_err(|_| unavailable())? = None;
        #[cfg(test)]
        {
            diagnostic.stage = "reserved_recover_member";
        }
        self.recover_member(stored, slot).await?;
        #[cfg(test)]
        {
            diagnostic.completed = true;
        }
        Ok(true)
    }
    pub(super) async fn recover_member(
        &self,
        mut stored: StoredRecord,
        mut slot: Slot,
    ) -> Result<(), HostLifetimeError> {
        let mut pending = slot.member_pending.clone().ok_or_else(invalid)?;
        let reservation = if pending.admission.phase == MemberClassAdmissionPhase::Retired {
            None
        } else {
            match self.reserve_member(&pending.admission, &pending.candidate) {
                Ok(reservation) => Some(reservation),
                Err(HostLifetimeError::Denied) => None,
                Err(error) => return Err(error),
            }
        };
        if reservation.is_none() && pending.admission.phase != MemberClassAdmissionPhase::Retired {
            // No authority to publish this instance. Retire only its concrete
            // decision using a successor that dominates either native outcome
            // of the previously retained pending addition. Existing members
            // and their grants remain exact.
            let mut retired = pending.admission.clone();
            retired.phase = MemberClassAdmissionPhase::Retired;
            pending = MemberPolicyDecision {
                candidate: self.member_candidate(&slot.current, &retired)?,
                prior: slot.current.clone(),
                admission: retired.clone(),
            };
            *slot
                .admissions
                .iter_mut()
                .find(|a| a.id == retired.id)
                .ok_or_else(invalid)? = retired.clone();
            slot.member_pending = Some(pending.clone());
            stored = self
                .change(&stored, &slot, "member-policy-retiring")
                .await?;
        }
        if reservation.is_some()
            && let Some(decision) = &slot.decision
        {
            self.recovery_reservation(
                decision,
                decision.proposal.candidate_canonical_json.as_bytes(),
                &slot,
            )?
            .check_current()?;
        }
        self.provider
            .accept_canonical_json(pending.candidate.as_bytes())
            .map_err(|_| unavailable())?;
        slot.current = pending.candidate.clone();
        slot.member_decision = Some(pending);
        slot.member_pending = None;
        self.change(&stored, &slot, "member-policy-current").await?;
        self.activate_current(&slot)?;
        // Revocation after native acceptance must also settle as a narrowing
        // successor, not leave an expired per-instance grant accepted.
        let recheck = reservation.as_ref().map(|r| r.check_current()).transpose();
        if let Err(error) = recheck {
            if error != HostLifetimeError::Denied {
                return Err(error);
            }
            let (stored, mut slot) = self.read().await?;
            let mut retired = slot
                .member_decision
                .as_ref()
                .ok_or_else(invalid)?
                .admission
                .clone();
            retired.phase = MemberClassAdmissionPhase::Retired;
            let candidate = self.member_candidate(&slot.current, &retired)?;
            *slot
                .admissions
                .iter_mut()
                .find(|a| a.id == retired.id)
                .ok_or_else(invalid)? = retired.clone();
            slot.member_pending = Some(MemberPolicyDecision {
                admission: retired,
                prior: slot.current.clone(),
                candidate,
            });
            let stored = self
                .change(&stored, &slot, "member-policy-retiring")
                .await?;
            *self.observed_current.lock().map_err(|_| unavailable())? = None;
            let pending = slot.member_pending.take().ok_or_else(invalid)?;
            self.provider
                .accept_canonical_json(pending.candidate.as_bytes())
                .map_err(|_| unavailable())?;
            slot.current = pending.candidate.clone();
            slot.member_decision = Some(pending);
            self.change(&stored, &slot, "member-policy-current").await?;
            self.activate_current(&slot)?;
        }
        Ok(())
    }

    /// Commit the authenticated route before the application's desired row is
    /// listed. Prepared routes are never projected as Active source authority.
    pub(crate) async fn commit_member_route(
        &self,
        id: &toolkit_package::Id,
        context: &meerkat_authorization_contracts::evidence::EvidenceId,
        route: meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
        standing_identity: Option<&meerkat_mobkit::identity_first::AgentIdentity>,
    ) -> Result<(), HostLifetimeError> {
        use meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin;
        self.recover().await?;
        let _exclusion = self.exclusion.lock().await;
        let (mut stored, mut slot) = self.read().await?;
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        let selected = slot
            .admissions
            .iter()
            .find(|a| &a.id == id)
            .ok_or(HostLifetimeError::Denied)?;
        if !matches!(&selected.phase, MemberClassAdmissionPhase::Prepared { context: prepared, context_generation }
            if prepared == context && selected.destination.as_ref().is_some_and(|destination|
                destination.context_generation == *context_generation))
            || selected
                .destination
                .as_ref()
                .is_none_or(|selected| &selected.context != context)
            || !matches!(&route.origin, CompiledNativeSourceOrigin::LocalConsole { member, .. }
                if member == &selected.instance.member.member)
            || selected
                .workflow
                .as_ref()
                .is_some_and(|binding| binding.original.route != route.id)
            || slot
                .admissions
                .iter()
                .any(|a| a.id != *id && a.route.as_ref().is_some_and(|prior| prior.id == route.id))
        {
            return Err(HostLifetimeError::Denied);
        }
        {
            let registrations = self
                .member_registrations
                .lock()
                .map_err(|_| unavailable())?;
            let registration = registrations
                .get(&selected.instance.member.member.member)
                .ok_or(HostLifetimeError::Denied)?;
            if registration.admission != *id
                || registration.registration.has_ended()
                || selected.destination.as_ref() != Some(&registration.destination)
            {
                return Err(HostLifetimeError::Denied);
            }
        }
        let mut prepared = selected.clone();
        prepared.route = Some(route);
        let reservation = self.reserve_member(&prepared, &slot.current)?;
        reservation.check_current()?;
        if let Some(identity) = standing_identity {
            let intent = self.standing_intent(&prepared, identity)?;
            if prepared.standing_operations.as_ref().is_some_and(|prior| {
                let mut prior = prior.clone();
                prior.issued = None;
                prior != intent
            }) {
                return Err(HostLifetimeError::Denied);
            }
            if prepared.standing_operations.is_none() {
                prepared.standing_operations = Some(intent);
                self.reserve_member(&prepared, &slot.current)?
                    .check_current()?;
                *slot
                    .admissions
                    .iter_mut()
                    .find(|a| &a.id == id)
                    .ok_or_else(invalid)? = prepared.clone();
                // The retained host Add task owns this exclusion across both
                // writes. Partial issuance leaves exact IDs, never fresh IDs.
                stored = self
                    .change(&stored, &slot, "member-standing-intent")
                    .await?;
            }
            reservation.check_current()?;
            let issued = self.issue_standing_parents(&prepared)?;
            let standing = prepared.standing_operations.as_mut().ok_or_else(invalid)?;
            if standing
                .issued
                .as_ref()
                .is_some_and(|prior| prior != &issued)
            {
                return Err(HostLifetimeError::Denied);
            }
            standing.issued = Some(issued);
        }
        self.reserve_member(&prepared, &slot.current)?
            .check_current()?;
        *slot
            .admissions
            .iter_mut()
            .find(|a| &a.id == id)
            .ok_or_else(invalid)? = prepared;
        self.change(&stored, &slot, "member-route-prepared").await?;
        reservation.check_current()?;
        self.activate_current(&slot)
    }

    /// The held-add owner calls this only after its actual Created outcome.
    /// The exact registration must still be the one selected before that
    /// construction, not merely another registration with the same session id.
    pub(crate) async fn activate_member(
        &self,
        id: &toolkit_package::Id,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Result<(), HostLifetimeError> {
        self.recover().await?;
        let _exclusion = self.exclusion.lock().await;
        let (stored, mut slot) = self.read().await?;
        if slot.pending.is_some() || slot.member_pending.is_some() {
            return Err(unavailable());
        }
        let selected = slot
            .admissions
            .iter_mut()
            .find(|a| &a.id == id)
            .ok_or(HostLifetimeError::Denied)?;
        let destination = self
            .expected_registration(
                &self.member_realm()?,
                &selected.instance.member.member,
                registration,
            )?
            .ok_or(HostLifetimeError::Denied)?;
        if selected.route.is_none()
            || selected.native_surface.is_none()
            || selected
                .standing_operations
                .as_ref()
                .is_some_and(|standing| standing.issued.is_none())
            || !matches!(&selected.phase,
                MemberClassAdmissionPhase::Prepared { context, context_generation }
                | MemberClassAdmissionPhase::Active { context, context_generation }
                if context == &destination.context && *context_generation == destination.context_generation)
        {
            return Err(HostLifetimeError::Denied);
        }
        selected.phase = MemberClassAdmissionPhase::Active {
            context: destination.context,
            context_generation: destination.context_generation,
        };
        let member = selected.instance.member.member.clone();
        let reservation = self.reserve_member(selected, &slot.current)?;
        self.change(&stored, &slot, "member-published").await?;
        reservation.check_current()?;
        self.activate_current(&slot)?;
        self.mark_registration_verified(&self.member_realm()?, &member, registration)
    }
    fn current_member_route(
        &self,
        id: &toolkit_package::Id,
    ) -> Result<
        Option<(
            MemberClassAdmission,
            meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
        )>,
        HostLifetimeError,
    > {
        self.check_current()?;
        let snapshot = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        let Some(record) = snapshot
            .admissions
            .iter()
            .find(|a| a.route.as_ref().is_some_and(|route| &route.id == id))
        else {
            return Ok(None);
        };
        let current = self
            .observed_admission(&record.instance.member.member)?
            .ok_or_else(unavailable)?;
        // The canonical owner may have advanced between the two reads. An
        // old requested route must never resolve to another admission or route.
        if current.id != record.id || current.route.as_ref().is_none_or(|route| &route.id != id) {
            return Err(HostLifetimeError::Denied);
        }
        if matches!(&current.phase, MemberClassAdmissionPhase::Prepared { context, context_generation }
            if current.destination.as_ref().is_some_and(|destination| &destination.context == context
                && destination.context_generation == *context_generation))
        {
            return Ok(None);
        }
        if !matches!(&current.phase, MemberClassAdmissionPhase::Active { context, context_generation }
            if current.destination.as_ref().is_some_and(|destination| &destination.context == context
                && destination.context_generation == *context_generation))
        {
            return Err(HostLifetimeError::Denied);
        }
        let route = current.route.clone().ok_or_else(unavailable)?;
        Ok(Some((current, route)))
    }

    pub(crate) fn member_route(
        &self,
        id: &toolkit_package::Id,
    ) -> Result<
        Option<(
            MemberClassAdmission,
            meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
        )>,
        HostLifetimeError,
    > {
        let Some((current, route)) = self.current_member_route(id)? else {
            return Ok(None);
        };
        // Canonical phase names the verified destination. The process-only
        // correlation additionally fences a native registration that has ended;
        // a cold owner has no such evidence until the real registration hook.
        let registrations = self
            .member_registrations
            .lock()
            .map_err(|_| unavailable())?;
        let selected = registrations
            .get(&current.instance.member.member.member)
            .ok_or(HostLifetimeError::Denied)?;
        if selected.admission != current.id
            || selected.registration.has_ended()
            || selected.verified.as_ref() != Some(&selected.registration)
            || current.destination.as_ref() != Some(&selected.destination)
            || selected.destination.context.as_str()
                != selected.registration.session_id().to_string()
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(Some((current, route)))
    }
    /// [`Self::member_route`] for the recipient of a saved native selection.
    /// A recipient this process holds any registration for answers through
    /// that live path unchanged, so a partial, unverified, ended or replaced
    /// registration stays refused and is never treated as absent. A recipient
    /// with no process registration at all (Dormant after a restart, until a
    /// live operation wakes it) is read from its canonical admission instead,
    /// as [`Self::cold_ordinary_source_coordinates`] reads a cold ordinary
    /// member: Active, with the exact destination generation and session
    /// document its phase names. A retired, non-Active, unknown or changed
    /// recipient is refused as before. This read wakes nothing and stands for
    /// no registration.
    pub(crate) fn recipient_member_route(
        &self,
        id: &toolkit_package::Id,
    ) -> Result<
        Option<(
            MemberClassAdmission,
            meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
        )>,
        HostLifetimeError,
    > {
        let Some((current, route)) = self.current_member_route(id)? else {
            return Ok(None);
        };
        let present = self
            .member_registrations
            .lock()
            .map_err(|_| unavailable())?
            .contains_key(&current.instance.member.member.member);
        if present {
            return self.member_route(id);
        }
        Ok(Some((current, route)))
    }
    /// Only a genuinely absent process registration can take the cold ingress
    /// path. A partial, ended or replaced registration still needs the ordinary
    /// strict route check; none can be reclassified as a cold owner.
    pub(crate) fn cold_ordinary_source_coordinates(
        &self,
        id: &toolkit_package::Id,
    ) -> Result<Option<ColdOrdinaryMemberSourceCoordinates>, HostLifetimeError> {
        let Some((admission, route)) = self.current_member_route(id)? else {
            return Ok(None);
        };
        if !admission.class.purpose.is_ordinary() || admission.workflow.is_some() {
            return Ok(None);
        }
        let present = self
            .member_registrations
            .lock()
            .map_err(|_| unavailable())?
            .contains_key(&admission.instance.member.member.member);
        if present {
            // Reuse every exact live-registration predicate; do not copy it.
            self.member_route(id)?.ok_or(HostLifetimeError::Denied)?;
            return Ok(None);
        }
        Ok(Some(ColdOrdinaryMemberSourceCoordinates {
            admission,
            route,
        }))
    }

    pub(crate) fn member_routes(
        &self,
    ) -> Result<
        Vec<(
            MemberClassAdmission,
            meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
        )>,
        HostLifetimeError,
    > {
        self.check_current()?;
        let snapshot = self
            .observed_current
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        let mut routes = Vec::new();
        for record in &snapshot.admissions {
            if !matches!(record.phase, MemberClassAdmissionPhase::Active { .. }) {
                continue;
            }
            if let Some(route) = &record.route {
                match self.member_route(&route.id) {
                    Ok(Some(selected)) => routes.push(selected),
                    Err(HostLifetimeError::Denied) | Ok(None) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(routes)
    }
    pub(crate) async fn retire_member(
        &self,
        id: &toolkit_package::Id,
    ) -> Result<(), HostLifetimeError> {
        self.retire_member_selected(id, None).await
    }
    pub(crate) async fn retire_member_exact(
        &self,
        id: &toolkit_package::Id,
        member: &MobMemberBinding,
    ) -> Result<(), HostLifetimeError> {
        self.retire_member_selected(id, Some(member)).await
    }
    async fn retire_member_selected(
        &self,
        id: &toolkit_package::Id,
        member: Option<&MobMemberBinding>,
    ) -> Result<(), HostLifetimeError> {
        self.recover().await?;
        let _exclusion = self.exclusion.lock().await;
        let (stored, mut slot) = self.read().await?;
        let mut retired = slot
            .admissions
            .iter()
            .find(|a| &a.id == id)
            .ok_or(HostLifetimeError::Denied)?
            .clone();
        if member.is_some_and(|member| member != &retired.instance.member.member) {
            return Err(HostLifetimeError::Denied);
        }
        if retired.phase == MemberClassAdmissionPhase::Retired {
            self.revoke_class_callbacks(&retired)?;
            return Ok(());
        }
        retired.phase = MemberClassAdmissionPhase::Retired;
        let candidate = self.member_candidate(&slot.current, &retired)?;
        *slot
            .admissions
            .iter_mut()
            .find(|a| &a.id == id)
            .ok_or_else(invalid)? = retired.clone();
        slot.member_pending = Some(MemberPolicyDecision {
            admission: retired.clone(),
            prior: slot.current.clone(),
            candidate,
        });
        let stored = self
            .change(&stored, &slot, "member-policy-retiring")
            .await?;
        *self.observed_current.lock().map_err(|_| unavailable())? = None;
        self.recover_member(stored, slot).await?;
        self.revoke_class_callbacks(&retired)?;
        self.member_registrations
            .lock()
            .map_err(|_| unavailable())?
            .retain(|_, selected| selected.admission != *id);
        Ok(())
    }
}

mod callbacks;
mod standing;
#[cfg(test)]
mod tests;
