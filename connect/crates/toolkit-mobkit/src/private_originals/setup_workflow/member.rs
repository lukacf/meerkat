//! Host-owned member selection before full-roster bootstrap. Fresh members use
//! held completion; idle admitted members can restore exact historical custody.
//! This is not an append operation on an active roster.
use super::*;
use crate::original_ingress::workflow_member::SelectedWorkflowMember;
use meerkat_mobkit::identity_first::{AgentAddressability, DurableAgentSpec, HeldIdentityMember};
use meerkat_toolkit::setup::SetupWorkflowTerms;

/// An opaque restrictive selection owned by this application's canonical
/// workflow/source owner. Native registration and held completion supply the
/// actual session and controller observation. It conveys no planning grant.
/// The host must supply a compiled InternalOnly member. Fresh members require
/// held completion. Ordinary resumed members can restore restricted historical
/// source custody from exact admitted evidence. Several unrelated workflows may
/// reserve distinct compiled members in the same host.
pub struct InstalledWorkflowMember {
    source: Weak<PrivateOriginals>,
    reference: SetupWorkflowRef,
    original: PrivateOriginalRef,
    member: MobMemberBinding,
}

/// Restrictive preparation on the same source owner before workflow terms and
/// an installed lifetime exist. Only actual native registration captures its
/// destination; later binding cannot replace that capture or create authority.
pub struct PreparedWorkflowMember {
    source: Weak<PrivateOriginals>,
    route: Id,
    original: meerkat_authorization_contracts::work_association::OriginalWorkRef,
    member: MobMemberBinding,
    spec: DurableAgentSpec,
}

impl PreparedWorkflowMember {
    /// Bind the final workflow to this preselected member after the same
    /// prepared owner adopts its installed lifetime. No hook is replayed.
    pub async fn bind(
        &self,
        lifetime: &Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
        terms: &SetupWorkflowTerms,
        original: &PrivateOriginalRef,
    ) -> Result<Arc<InstalledWorkflowMember>, PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let installed = source
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !Arc::ptr_eq(&installed, lifetime) {
            return Err(PrivateOriginalError::Denied);
        }
        if original.route != self.route
            || original.original != self.original.work
            || terms.lead != self.member
        {
            return Err(PrivateOriginalError::Conflict);
        }
        source
            .owner
            .check_prepared_workflow_member_capture(&self.member, &self.original)
            .map_err(custody_error)?;
        source
            .reserve_workflow_member_for_host(lifetime, terms, original, &self.spec)
            .await
    }

    pub(crate) fn selected_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        destination: &meerkat_mob::MemberDestinationSelection,
    ) -> Result<(), meerkat_mob::MobError> {
        self.source
            .upgrade()
            .ok_or_else(|| {
                meerkat_mob::MobError::WiringError(
                    "workflow preparation owner is unavailable".into(),
                )
            })?
            .owner
            .capture_prepared_workflow_member(&self.member, &self.original, identity, destination)
            .map_err(|_| {
                meerkat_mob::MobError::WiringError("workflow preparation capture refused".into())
            })
    }
}

fn valid_spec(spec: &DurableAgentSpec, member: &MobMemberBinding) -> bool {
    meerkat_mobkit::member_comms_id::mob_member_id(spec.identity.as_str()).as_str() == member.member
        && spec.addressability == AgentAddressability::InternalOnly
        && spec.context.is_none()
        && spec.initial_message.is_none()
        && spec.binding.is_none()
        && spec.placement.is_none()
}

impl PrivateOriginals {
    pub(crate) async fn prepare_workflow_member_for_host(
        self: &Arc<Self>,
        selected: &meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy,
        route: &Id,
        original: &EvidenceId,
        spec: &DurableAgentSpec,
    ) -> Result<Arc<PreparedWorkflowMember>, PrivateOriginalError> {
        let native = &selected.native_host;
        if self.scope.realm != native.realm.to_string()
            || self.scope.owner != selected.deployment.to_string()
            || self.owner.ingress_domain() != &native.ingress_domain
            || self.host_lifetime.get().is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let compiled = native
            .routes
            .iter()
            .find(|row| &row.id == route)
            .ok_or(PrivateOriginalError::Denied)?;
        let (member, _, _) = compiled
            .source_binding_in(&selected.channels)
            .map_err(|_| PrivateOriginalError::Denied)?;
        if !valid_spec(spec, member) {
            return Err(PrivateOriginalError::Denied);
        }
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: compiled.source_authority.clone(),
            work: original.clone(),
        };
        self.owner
            .select_workflow_member(
                SelectedWorkflowMember {
                    class_admission: None,
                    reference: None,
                    member: member.clone(),
                    original: original.clone(),
                    realm: native.realm.clone(),
                    identity: spec.identity.clone(),
                    captured: None,
                },
                || Ok(()),
                || Ok(()),
            )
            .await
            .map_err(custody_error)?;
        Ok(Arc::new(PreparedWorkflowMember {
            source: Arc::downgrade(self),
            route: route.clone(),
            original,
            member: member.clone(),
            spec: spec.clone(),
        }))
    }

    // Only the composed gateway reserves before native member creation. The
    // existing source lifetime is selected atomically with its restriction.
    pub(crate) async fn reserve_workflow_member_for_host(
        self: &Arc<Self>,
        lifetime: &Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
        terms: &SetupWorkflowTerms,
        original: &PrivateOriginalRef,
        spec: &DurableAgentSpec,
    ) -> Result<Arc<InstalledWorkflowMember>, PrivateOriginalError> {
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let selected = lifetime.selected_policy();
        let native = selected
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if self.scope.realm != native.realm.to_string()
            || self.scope.owner != selected.deployment.to_string()
            || self.owner.ingress_domain() != &native.ingress_domain
        {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some(bound) = self.host_lifetime.get() {
            let bound = bound.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
            if !Arc::ptr_eq(&bound, lifetime) {
                return Err(PrivateOriginalError::Denied);
            }
        }
        let reference = self
            .workflow_store()?
            .workflow_ref(terms)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let route = self.route(&original.route)?;
        let event = self.event(original).await?;
        let compiled = native
            .routes
            .iter()
            .find(|row| row.id == original.route)
            .ok_or(PrivateOriginalError::Denied)?;
        let (member, requester, actor) = compiled
            .source_binding(selected)
            .map_err(|_| PrivateOriginalError::Denied)?;
        if terms.planning_source != self.key(original)?
            || member != &terms.lead
            || requester != &route.requester
            || actor != &route.ingress_actor
            || terms.requester != route.requester
            || terms.realm != route.realm
            || route.grant_target.as_str() != member.member
            || !valid_spec(spec, member)
            || event.contributors.len() != 1
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .select_workflow_member(
                SelectedWorkflowMember {
                    class_admission: None,
                    reference: Some(reference.clone()),
                    member: member.clone(),
                    realm: route.realm.clone(),
                    original: meerkat_authorization_contracts::work_association::OriginalWorkRef {
                        authority: route.source_authority.clone(),
                        work: original.original.clone(),
                    },
                    identity: spec.identity.clone(),
                    captured: None,
                },
                || {
                    lifetime
                        .validate_current_selection()
                        .map_err(|_| OriginalWriterError::Unavailable)
                },
                || {
                    self.bind_host_lifetime(lifetime)
                        .map_err(|error| match error {
                            PrivateOriginalError::Unavailable => OriginalWriterError::Unavailable,
                            _ => OriginalWriterError::WrongOwner,
                        })
                },
            )
            .await
            .map_err(custody_error)?;
        Ok(Arc::new(InstalledWorkflowMember {
            source: Arc::downgrade(self),
            reference,
            original: original.clone(),
            member: member.clone(),
        }))
    }
}
impl InstalledWorkflowMember {
    pub(in crate::private_originals) fn from_completed_class(
        source: &Arc<PrivateOriginals>,
        admission: &crate::installed_host::dynamic_members::MemberClassAdmission,
    ) -> Result<Arc<Self>, PrivateOriginalError> {
        let binding = admission
            .workflow
            .as_ref()
            .ok_or(PrivateOriginalError::Denied)?;
        let result = Arc::new(Self {
            source: Arc::downgrade(source),
            reference: binding.workflow.clone(),
            original: binding.original.clone(),
            member: admission.instance.member.member.clone(),
        });
        // Existing native capture and held controller evidence must both exist.
        result.session()?;
        Ok(result)
    }
    /// Select this cohort's configured acquisition A for exact bound Input
    /// retention before its first binding. The same selection is idempotent;
    /// already-bound unselected inputs cannot be retrofitted. Ordinary bodies
    /// remain with their application. Live Console login restoration and helper
    /// effect targets are outside this host-submitted SDK profile.
    /// This selects evidence only, not disclosure permission or a new grant.
    pub async fn prepare_waiter_source(
        &self,
        original: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        source
            .select_workflow_waiter_evidence(
                &self.reference,
                &self.original,
                &self.member,
                original,
                false,
            )
            .await
    }

    /// Select fresh explicit shared-effect evidence before the original's first
    /// binding. This requires the separately reviewed workflow target profile;
    /// it does not retrofit Find-only inputs or grant G its own authority.
    pub async fn prepare_effect_waiter_source(
        &self,
        original: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        source
            .select_workflow_waiter_evidence(
                &self.reference,
                &self.original,
                &self.member,
                original,
                true,
            )
            .await
    }

    /// Restore exact selected A Task custody after ordinary native resume.
    /// Find-only and shared-effect inputs retain their historical basis. No
    /// input or grant is issued and missing evidence stays unavailable. The
    /// separate interest restore recovers only previously issued effect leaves;
    /// every selected operation still checks its own current native authority.
    pub async fn restore_waiter_source(
        &self,
        original: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        source
            .restore_workflow_waiter_source(&self.reference, &self.original, &self.member, original)
            .await
    }

    /// Reattach one exact canonical live interest after explicit A and idle
    /// P/G source restoration. Reuses their actual native custody and original
    /// input; it does not resolve a request, launch G or issue a Find/delegation.
    /// Completing the G controller also installs the restrictive workflow with
    /// no waiter effect custody. A host must separately select any authorized
    /// context transfer; this is not process-loss delivery reconstruction.
    pub async fn restore_waiter_interest(
        &self,
        original: &PrivateOriginalRef,
        membership: &SetupWorkflowWaiter,
    ) -> Result<Arc<InstalledWorkflowPlanning>, PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        source
            .restore_workflow_interest(
                &self.reference,
                &self.original,
                &self.member,
                original,
                membership,
            )
            .await
    }

    /// Compose this participant with the host's existing registration hook.
    /// It observes that hook's actual selection before native installation.
    pub(crate) fn registration_participant(
        self: &Arc<Self>,
    ) -> crate::installed_host::prepared_registration::SelectedDestinationParticipant {
        let selected = self.clone();
        Arc::new(move |identity, destination| {
            let source = selected.source.upgrade().ok_or_else(|| {
                meerkat_mob::MobError::WiringError("workflow source owner is unavailable".into())
            })?;
            source
                .owner
                .capture_workflow_member(&selected.reference, identity, destination)
                .map_err(|_| {
                    meerkat_mob::MobError::WiringError("workflow member selection refused".into())
                })
        })
    }

    /// Called by the real held full-roster completion callback before release.
    /// The report constrains the later actual native client; it grants nothing.
    pub async fn complete_held_member(
        &self,
        held: &HeldIdentityMember,
    ) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        source
            .owner
            .finish_workflow_member(&self.reference, held)
            .await
            .map_err(custody_error)
    }

    /// Restore an idle G's historical source after ordinary native roster resume.
    /// Actual registration must have captured the saved session and destination.
    /// The canonical workflow and explicitly retained P/G inputs are rechecked;
    /// native supplies G's historical admission and current controller custody.
    /// Missing evidence or a fresh fallback cannot complete this selection.
    /// This never sends an input, reconstructs waiters or starts recovery work.
    /// The captured controller remains incomplete: native admission and operation
    /// policy stay unavailable. A later exact workflow attachment must install
    /// its retained waiter/operation custody and complete that controller together;
    /// that restart attachment path is not implemented by this method.
    pub async fn restore_admitted_source(&self) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        source
            .restore_workflow_lead_source(&self.reference, &self.original, &self.member)
            .await
    }

    /// Exact captured native session, available only after native completion.
    /// This observation is not a live-session or exclusive-run capability.
    pub fn session(&self) -> Result<meerkat_core::SessionId, PrivateOriginalError> {
        self.source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?
            .owner
            .dedicated_workflow_session(&self.reference, &self.member, &self.original.original)
            .map_err(custody_error)?
            .ok_or(PrivateOriginalError::Unavailable)
    }
}
