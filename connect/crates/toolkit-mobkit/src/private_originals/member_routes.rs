//! Read projections of the same canonical member admission. Static installed
//! routes remain immutable; a class route is available only after native held
//! creation and current owner publication, never from the packaged profile.
use super::*;
use crate::installed_host::dynamic_members::{MemberClassAdmission, MemberClassAdmissionPhase};
use meerkat_toolkit::host_lifetime::policy::{
    CompiledNativeSourceOrigin, CompiledNativeSourceRoute, PreparedNativeMember,
};

/// Which member route a selection resolves through.
#[derive(Clone, Copy)]
enum MemberRouteRead {
    /// The live route: a member with a verified registration in this process.
    Live,
    /// The recipient of a saved native selection: also a member with no
    /// process registration at all, from its canonical Active admission.
    Recipient,
}

impl MemberRouteRead {
    fn member_route(
        self,
        owner: &crate::installed_host::tool_policy::service_policy::ServicePolicyOwner,
        id: &Id,
    ) -> Result<
        Option<(MemberClassAdmission, CompiledNativeSourceRoute)>,
        meerkat_toolkit::host_lifetime::control::HostLifetimeError,
    > {
        match self {
            Self::Live => owner.member_route(id),
            Self::Recipient => owner.recipient_member_route(id),
        }
    }
}

impl PrivateOriginals {
    /// Ordinary class Add is rooted in the installed operator, not in the
    /// application's requested principal fields. Issued parents are deliberately
    /// permitted here so the same facts can be checked on retained Active routes.
    pub(crate) fn check_operator_member_route(
        &self,
        selected: &PreparedNativeMember,
        route: &CompiledNativeSourceRoute,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
    ) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        lifetime.validate_current_selection()?;
        let native = lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        let operator = native
            .remote_operator
            .as_ref()
            .ok_or(HostLifetimeError::Denied)?;
        let ingress = &native.ingress_domain.authority;
        let authentication = native
            .authentication_domain
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        let event = native
            .event_domain
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        let CompiledNativeSourceOrigin::LocalConsole {
            member,
            requester,
            ingress_actor,
        } = &route.origin
        else {
            return Err(HostLifetimeError::Denied);
        };
        let mut executor =
            PrincipalRef::new(meerkat_core::PrincipalKind::SharedAgent, identity.as_str())
                .map_err(|_| HostLifetimeError::Denied)?;
        executor.qualification = ingress.qualification.clone();
        executor
            .validate_qualified()
            .map_err(|_| HostLifetimeError::Denied)?;
        let alias =
            meerkat_mobkit::member_comms_id::roster_member_id_for_identity(identity.as_str());
        if requester != operator
            || route.represented_subject.as_ref() != Some(operator)
            || ingress_actor != ingress
            || &route.source_authority != ingress
            || route.logical_executor != executor
            || selected.logical_owner != executor
            || selected.audience
                != (meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                    principal: operator.clone(),
                })
            || member != &selected.member
            || member.member != alias.as_str()
            || &route.authentication_domain != authentication
            || &route.event_domain != event
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    /// Capture the actual ordinary class admission for this setup-only use.
    /// Static callers retain their existing immutable native selection.
    pub(crate) fn selected_setup_admission(
        &self,
        selected: &(
            CompiledNativeSourceRoute,
            PreparedNativeMember,
            Option<EvidenceId>,
        ),
    ) -> Result<
        Option<MemberClassAdmission>,
        meerkat_toolkit::host_lifetime::control::HostLifetimeError,
    > {
        self.setup_admission_through(selected, MemberRouteRead::Live)
    }

    /// [`Self::selected_setup_admission`] for a saved selection's recipient:
    /// a recipient with no process registration is read from its canonical
    /// Active admission ([`MemberRouteRead::Recipient`]).
    pub(super) fn recipient_setup_admission(
        &self,
        selected: &(
            CompiledNativeSourceRoute,
            PreparedNativeMember,
            Option<EvidenceId>,
        ),
    ) -> Result<
        Option<MemberClassAdmission>,
        meerkat_toolkit::host_lifetime::control::HostLifetimeError,
    > {
        self.setup_admission_through(selected, MemberRouteRead::Recipient)
    }

    fn setup_admission_through(
        &self,
        selected: &(
            CompiledNativeSourceRoute,
            PreparedNativeMember,
            Option<EvidenceId>,
        ),
        read: MemberRouteRead,
    ) -> Result<
        Option<MemberClassAdmission>,
        meerkat_toolkit::host_lifetime::control::HostLifetimeError,
    > {
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        if selected.2.is_none() {
            return Ok(None);
        }
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        let (admission, route) = read
            .member_route(owner, &selected.0.id)?
            .ok_or(HostLifetimeError::Denied)?;
        let MemberClassAdmissionPhase::Active { context, .. } = &admission.phase else {
            return Err(HostLifetimeError::Denied);
        };
        if admission.workflow.is_some()
            || !admission.class.purpose.is_ordinary()
            || route != selected.0
            || admission.selected_member()? != selected.1
            || selected.2.as_ref() != Some(context)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(Some(admission))
    }

    /// Reuse the actual member-policy exclusion across synchronous setup grant
    /// issuance. The selected tuple is only compared, never used as permission.
    pub(crate) async fn with_selected_setup_caller<T>(
        &self,
        selected: &(
            CompiledNativeSourceRoute,
            PreparedNativeMember,
            Option<EvidenceId>,
        ),
        admission: Option<&MemberClassAdmission>,
        effect: impl FnOnce() -> Result<T, meerkat_toolkit::host_lifetime::control::HostLifetimeError>,
    ) -> Result<T, meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        let check = || {
            let current =
                self.selected_source_route(&selected.0.id)
                    .map_err(|error| match error {
                        PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
                        _ => HostLifetimeError::Denied,
                    })?;
            if &current != selected {
                return Err(HostLifetimeError::Denied);
            }
            if self.selected_setup_admission(selected)?.as_ref() != admission {
                return Err(HostLifetimeError::Denied);
            }
            Ok(())
        };
        check()?;
        if selected.2.is_none() {
            return effect();
        }
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        let admission = admission.ok_or(HostLifetimeError::Denied)?;
        owner
            .with_current_member(&admission.id, &admission.class, &admission.instance, || {
                check()?;
                effect()
            })
            .await
    }

    fn project_member_route(
        &self,
        admission: &MemberClassAdmission,
        route: &CompiledNativeSourceRoute,
    ) -> Result<PrivateOriginalRoute, PrivateOriginalError> {
        if !matches!(admission.phase, MemberClassAdmissionPhase::Active { .. }) {
            return Err(PrivateOriginalError::Denied);
        }
        let selected = admission
            .selected_member()
            .map_err(|_| PrivateOriginalError::Denied)?;
        if admission.class.purpose.is_ordinary() {
            if admission.workflow.is_some() {
                return Err(PrivateOriginalError::Denied);
            }
            let identity = meerkat_mobkit::identity_first::AgentIdentity::parse(
                selected.logical_owner.id.as_str(),
            )
            .map_err(|_| PrivateOriginalError::Denied)?;
            self.check_operator_member_route(&selected, route, &identity)
                .map_err(|error| match error {
                    meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable
                    | meerkat_toolkit::host_lifetime::control::HostLifetimeError::NotSettled => {
                        PrivateOriginalError::Unavailable
                    }
                    _ => PrivateOriginalError::Denied,
                })?;
        }
        self.project_class_source_route(&selected, route)
    }

    /// Pure source-fact projection. Only callers with native Active evidence
    /// publish it; workflow preparation can retain inert historical facts.
    pub(super) fn project_class_source_route(
        &self,
        selected: &PreparedNativeMember,
        route: &CompiledNativeSourceRoute,
    ) -> Result<PrivateOriginalRoute, PrivateOriginalError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let native = lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let CompiledNativeSourceOrigin::LocalConsole {
            member,
            requester,
            ingress_actor,
        } = &route.origin
        else {
            return Err(PrivateOriginalError::Denied);
        };
        if member != &selected.member
            || native.realm.as_str() != self.scope.realm
            || selected.setup_capability.is_some()
            || native
                .members
                .iter()
                .any(|row| row.member.member == member.member)
            || native.routes.iter().any(|row| row.id == route.id)
        {
            return Err(PrivateOriginalError::Denied);
        }
        let projected = PrivateOriginalRoute {
            id: route.id.clone(),
            grant_caller: native.grant_administrator.clone(),
            application_parent: route.application_parent.clone(),
            controller_parent: route.controller_parent.clone(),
            grant_target: Id::new(member.member.clone())
                .map_err(|_| PrivateOriginalError::Denied)?,
            grant_audience: selected.audience.clone(),
            realm: native.realm.clone(),
            requester: requester.clone(),
            ingress_actor: ingress_actor.clone(),
            logical_executor: route.logical_executor.clone(),
            represented_subject: route.represented_subject.clone(),
            source_authority: route.source_authority.clone(),
            authentication_domain: route.authentication_domain.clone(),
            event_domain: route.event_domain.clone(),
        };
        Self::validate_routes(&self.scope, std::slice::from_ref(&projected))?;
        Ok(projected)
    }

    pub(crate) fn current_routes(&self) -> Result<Vec<PrivateOriginalRoute>, PrivateOriginalError> {
        let mut routes = self.routes()?.to_vec();
        if let Some(owner) = self.service_tool_policy.get() {
            for (admission, route) in owner
                .member_routes()
                .map_err(|_| PrivateOriginalError::Unavailable)?
            {
                routes.push(self.project_member_route(&admission, &route)?);
            }
        }
        Self::validate_routes(&self.scope, &routes)?;
        Ok(routes)
    }

    /// Current ordinary source coordinates only. Static routes and live
    /// registrations continue through their existing strict route path.
    pub(super) fn cold_ordinary_source_coordinates(
        &self,
        id: &Id,
    ) -> Result<Option<cold_source_coordinates::ColdOrdinarySourceCoordinates>, PrivateOriginalError>
    {
        if self.routes()?.iter().any(|route| &route.id == id) {
            return Ok(None);
        }
        let Some(owner) = self.service_tool_policy.get() else {
            return Ok(None);
        };
        let Some(coordinates) = owner
            .cold_ordinary_source_coordinates(id)
            .map_err(|_| PrivateOriginalError::Unavailable)?
        else {
            return Ok(None);
        };
        let route = self.project_member_route(coordinates.admission(), coordinates.route())?;
        Ok(Some(
            cold_source_coordinates::ColdOrdinarySourceCoordinates::new(
                coordinates.admission().clone(),
                route,
            ),
        ))
    }

    pub(super) fn member_route(
        &self,
        id: &Id,
    ) -> Result<Option<PrivateOriginalRoute>, PrivateOriginalError> {
        let Some(owner) = self.service_tool_policy.get() else {
            return Ok(None);
        };
        owner
            .member_route(id)
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .map(|(admission, route)| self.project_member_route(&admission, &route))
            .transpose()
    }

    /// The current exact route and concrete destination. Only a dynamic native
    /// completion supplies a fixed session document; static behavior is kept.
    pub(crate) fn selected_source_route(
        &self,
        id: &Id,
    ) -> Result<
        (
            CompiledNativeSourceRoute,
            PreparedNativeMember,
            Option<EvidenceId>,
        ),
        PrivateOriginalError,
    > {
        self.source_route_through(id, MemberRouteRead::Live)
    }

    /// [`Self::selected_source_route`] for a saved selection's recipient: a
    /// recipient with no process registration is read from its canonical
    /// Active admission ([`MemberRouteRead::Recipient`]).
    pub(super) fn recipient_source_route(
        &self,
        id: &Id,
    ) -> Result<
        (
            CompiledNativeSourceRoute,
            PreparedNativeMember,
            Option<EvidenceId>,
        ),
        PrivateOriginalError,
    > {
        self.source_route_through(id, MemberRouteRead::Recipient)
    }

    fn source_route_through(
        &self,
        id: &Id,
        read: MemberRouteRead,
    ) -> Result<
        (
            CompiledNativeSourceRoute,
            PreparedNativeMember,
            Option<EvidenceId>,
        ),
        PrivateOriginalError,
    > {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let selected = lifetime.selected_policy();
        let native = selected
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if let Some(route) = native.routes.iter().find(|route| &route.id == id) {
            let (binding, _, _) = route
                .source_binding(selected)
                .map_err(|_| PrivateOriginalError::Denied)?;
            let member = native
                .members
                .iter()
                .find(|row| &row.member == binding)
                .ok_or(PrivateOriginalError::Denied)?;
            return Ok((
                route.clone(),
                PreparedNativeMember {
                    member: member.member.clone(),
                    logical_owner: member.logical_owner.clone(),
                    context_generation: member.context_generation,
                    audience: member.audience.clone(),
                    setup_capability: member.setup_capability.clone(),
                },
                None,
            ));
        }
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(PrivateOriginalError::Denied)?;
        let (admission, route) = read
            .member_route(owner, id)
            .map_err(|error| match error {
                meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable
                | meerkat_toolkit::host_lifetime::control::HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
                _ => PrivateOriginalError::Denied,
            })?
            .ok_or(PrivateOriginalError::Denied)?;
        self.project_member_route(&admission, &route)?;
        let MemberClassAdmissionPhase::Active { ref context, .. } = admission.phase else {
            return Err(PrivateOriginalError::Denied);
        };
        let member = admission
            .selected_member()
            .map_err(|_| PrivateOriginalError::Denied)?;
        Ok((route, member, Some(context.clone())))
    }
}

impl PrivateOriginals {
    pub(crate) fn check_member_class_owner(
        &self,
        admitted: &crate::installed_host::dynamic_members::AdmittedMemberClass,
    ) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(owner, &admitted.owner) {
            return Err(HostLifetimeError::WrongOwner);
        }
        let actual = owner
            .observed_admission(&admitted.instance().member.member)?
            .ok_or(HostLifetimeError::Denied)?;
        if actual.id != *admitted.admission_id()
            || actual.instance != *admitted.instance()
            || actual.class != *admitted.class()
            || actual.workflow.as_ref() != admitted.workflow()
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    pub(crate) async fn complete_member_class(
        &self,
        admitted: &crate::installed_host::dynamic_members::AdmittedMemberClass,
        held: &meerkat_mobkit::identity_first::HeldIdentityMember,
        mut route: CompiledNativeSourceRoute,
        intent: crate::installed_host::preparation::PreparedControllerIntent,
    ) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        use crate::installed_host::{
            application_operations,
            preparation::{PreparedControllerRequest, controller_parent},
        };
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        self.check_member_class_owner(admitted)?;
        let context = admitted.verify_held(held)?;
        if admitted.class().purpose.is_ordinary() {
            self.check_operator_member_route(&admitted.instance().member, &route, &held.identity)?;
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        lifetime.validate_current_selection()?;
        let native = lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        let operator = native
            .remote_operator
            .as_ref()
            .ok_or(HostLifetimeError::Denied)?;
        let observed = held
            .controller_route
            .report()
            .ok_or(HostLifetimeError::Unavailable)?;
        if intent
            .controller_route
            .as_ref()
            .is_some_and(|key| key != &observed.key)
        {
            return Err(HostLifetimeError::Denied);
        }
        let request = PreparedControllerRequest {
            target: intent.target,
            issuance_id: intent.issuance_id,
            controller_route: observed.key.clone(),
        };
        let expected =
            application_operations::parent_restrictions(application_operations::controller(
                &native.grant_administrator,
                &route.logical_executor,
                &admitted.instance().member.audience,
                &observed.key,
                application_operations::ControllerAction::Inference,
            ));
        route.controller_parent = Some(controller_parent(
            &self.owner,
            &native.grant_administrator,
            operator,
            route.represented_subject.as_ref(),
            &request,
            expected,
        )?);
        // Commit authenticated held evidence durably, but keep Prepared. Native
        // activation has not occurred and no ordinary route is projected yet.
        admitted
            .owner
            .commit_member_route(
                admitted.admission_id(),
                &context,
                route,
                (admitted.class().purpose.is_ordinary()
                    && admitted
                        .add_token
                        .as_ref()
                        .is_some_and(|token| token.claimed()))
                .then_some(&held.identity),
            )
            .await?;
        self.finish_workflow_class(admitted, held).await
    }
}

impl PrivateOriginals {
    pub(crate) async fn activate_member_class(
        &self,
        admitted: &crate::installed_host::dynamic_members::AdmittedMemberClass,
        binding: &meerkat_mobkit::identity_first::MemberRuntimeBinding,
    ) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        self.check_member_class_owner(admitted)?;
        let session = binding
            .session_id
            .as_ref()
            .ok_or(HostLifetimeError::Denied)?;
        let machine = self
            .machine
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        let registration = machine
            .current_session_registration_witness(session)
            .await
            .ok_or(HostLifetimeError::Denied)?;
        let workflow_route = if admitted.workflow().is_some() {
            let current = admitted
                .owner
                .observed_admission(&admitted.instance().member.member)?
                .ok_or(HostLifetimeError::Denied)?;
            let route = current.route.ok_or(HostLifetimeError::Denied)?;
            self.check_workflow_class_source(admitted, &route).await?;
            Some(route)
        } else {
            None
        };
        admitted
            .owner
            .activate_member(admitted.admission_id(), &registration)
            .await?;
        if let Some(route) = workflow_route {
            self.check_workflow_class_source(admitted, &route).await?;
        }
        Ok(())
    }
}
