//! First source binding runs inside the native identity owner's real binder.
//! Registration creates no original or operation mandate. The actual session
//! and pinned controller arrive from native; the installed host owns the rest.

use super::*;
use crate::installed_host::application_operations;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::{DelegationDepth, ExecutionRestrictions},
    protocol::ContractRequirements,
    work_association::{
        InputAuthorityAssociation, InputAuthorityAssociationCandidate, NativeWorkTarget,
        OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
    },
};
use meerkat_mobkit::identity_first::GovernedBindContext;
use meerkat_runtime::meerkat_machine::AuthenticatedOriginal;
use meerkat_toolkit::host_lifetime::{NativeHostLifetime, control::HostLifetimeError};

impl PrivateOriginals {
    /// Select the exact host once, or confirm that same live owner during
    /// bootstrap of that same selected host. This weak reverse
    /// reference cannot keep a host alive or transfer to a replacement host.
    pub(crate) fn bind_host_lifetime(
        &self,
        lifetime: &Arc<NativeHostLifetime>,
    ) -> Result<(), PrivateOriginalError> {
        let selected = lifetime.selected_policy();
        let native = selected.native_host.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        if self.scope.realm != native.realm.to_string()
            || self.scope.owner != selected.deployment.to_string()
            || self.owner.ingress_domain() != &native.ingress_domain
        {
            return Err(PrivateOriginalError::Denied);
        }
        // Concurrent confirmations inspect the actual winner. An expired
        // selection remains closed; equal serialized policy is not identity.
        let selected = self.host_lifetime.get_or_init(|| Arc::downgrade(lifetime))
            .upgrade().ok_or(PrivateOriginalError::Denied)?;
        if Arc::ptr_eq(&selected, lifetime) { Ok(()) }
        else { Err(PrivateOriginalError::Denied) }
    }

    pub(super) async fn binding_for_input(
        self: &Arc<Self>,
        reference: &PrivateOriginalRef,
        context: &GovernedBindContext,
    ) -> Result<Arc<HeldPrivateOriginal>, PrivateOriginalError> {
        self.binding_for_selected_input(reference, context, None, None, None, None).await
    }

    /// A separately authenticated input may select its already issued task
    /// grant. The old source is never rebound to this new permission.
    pub(super) async fn binding_for_access_input(
        self: &Arc<Self>,
        reference: &PrivateOriginalRef,
        context: &GovernedBindContext,
        grant: &PrivateServiceGrantRef,
        approved_login: Option<crate::private_host::ConsoleLoginHandle>,
    ) -> Result<Arc<HeldPrivateOriginal>, PrivateOriginalError> {
        self.binding_for_selected_input(reference, context, Some(grant), None, None, approved_login).await
    }

    pub(super) async fn binding_for_batch_input(
        self: &Arc<Self>, reference: &PrivateOriginalRef, context: &GovernedBindContext,
        task: &service_approved_input::ApprovedTaskLink,
        approved_login: Option<crate::private_host::ConsoleLoginHandle>,
    ) -> Result<Arc<HeldPrivateOriginal>, PrivateOriginalError> {
        self.binding_for_selected_input(reference, context, None, Some(task), None, approved_login).await
    }

    pub(super) async fn binding_for_console_input(
        self: &Arc<Self>, reference: &PrivateOriginalRef, context: &GovernedBindContext,
        entry: super::console_ingress::ConsoleEntry,
    ) -> Result<Arc<HeldPrivateOriginal>, PrivateOriginalError> {
        self.binding_for_selected_input(reference, context, None, None, Some(entry), None).await
    }


    async fn binding_for_selected_input(
        self: &Arc<Self>,
        reference: &PrivateOriginalRef,
        context: &GovernedBindContext,
        grant: Option<&PrivateServiceGrantRef>,
        batch: Option<&service_approved_input::ApprovedTaskLink>,
        console: Option<super::console_ingress::ConsoleEntry>,
        approved_login: Option<crate::private_host::ConsoleLoginHandle>,
    ) -> Result<Arc<HeldPrivateOriginal>, PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        if grant.is_some() && batch.is_some() { return Err(PrivateOriginalError::Conflict); }
        let selected_setup = if grant.is_none() && batch.is_none() {
            if record.setup_request.is_some() {
                // An explicit per-original review takes precedence. It is never
                // upgraded or replaced by configured agent acquisition.
                self.setup_request_basis(&record, reference, &context.session_id, console.clone()).await?
            } else {
                self.configured_setup_basis(&record, reference, &context.session_id, console.clone()).await?
            }
        } else {
            if record.setup_request.is_some() { return Err(PrivateOriginalError::Denied); }
            None
        };
        let selected_basis = if let Some(task) = batch {
            if let Some(setup) = task.setup() {
                self.issue_service_setup(reference, setup, approved_login.as_ref()).await?;
            }
            self.service_batch_basis(reference, task, true).await?.0
        } else { match grant {
            Some(grant) => {
                let route = self.route(&reference.route)?;
                let original = OriginalWorkRef {
                    authority: route.source_authority.clone(), work: reference.original.clone(),
                };
                WorkAuthorityBasis::GrantLineage {
                    lineage: self.service_grant_lineage(grant, &original).await?,
                }
            }
            None => match selected_setup.as_ref() {
                Some(basis) => basis.clone(),
                None => WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] },
            },
        }};
        if let Some(task) = &record.approved_task {
            if (match task {
                service_approved_input::ApprovedTaskLink::Single(member) => grant != Some(&member.grant) || batch.is_some(),
                service_approved_input::ApprovedTaskLink::Batch(_) => batch != Some(task) || grant.is_some(),
            }) || context.session_id != task.original_task.origin_session_id
                || !self.service_task_submission_current(&record).await?
            { return Err(PrivateOriginalError::Denied); }
        } else if grant.is_some() || batch.is_some() { return Err(PrivateOriginalError::Denied); }
        if let Some(association) = &record.final_association {
            if association.candidate().authority_basis != selected_basis {
                return Err(PrivateOriginalError::Conflict);
            }
            // The canonical row, never today's selection, chooses replay's
            // immutable controller and operation basis. Existing custody is
            // shared even when an equivalent source route alias was used.
            if let Some(held) = self.retained.lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .iter().find(|held| held.original.association() == association).cloned()
            {
                return Ok(held);
            }
        }
        if record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Unavailable)?;
        let selected = lifetime.selected_policy();
        let native = selected.native_host.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let route = native.routes.iter().find(|route| route.id == reference.route)
            .ok_or(PrivateOriginalError::Denied)?;
        let (binding, requester, actor) = route.source_binding(selected)
            .map_err(|_| PrivateOriginalError::Denied)?;
        let member = native.members.iter().find(|member| &member.member == binding)
            .ok_or(PrivateOriginalError::Denied)?;
        let source_route = self.route(&reference.route)?;
        if source_route.grant_target.as_str() != binding.member
            || source_route.grant_caller != native.grant_administrator
            || &record.source.requester != requester || &record.source.ingress_actor != actor
            || record.source.logical_executor != route.logical_executor
            || record.source.represented_subject != route.represented_subject
            || record.source.source_authority != route.source_authority
            || record.source.authentication_domain != route.authentication_domain
            || record.source.event_domain != route.event_domain
        {
            return Err(PrivateOriginalError::Conflict);
        }
        // This is the exact native registration selected by the tracked
        // identity call. Do not install or rebind its destination here. Native
        // registration and admission retain/check that destination themselves.
        let machine = self.machine()?;
        let registration = machine.current_session_registration_witness(&context.session_id)
            .await.ok_or(PrivateOriginalError::Unavailable)?;
        let target = NativeWorkTarget {
            logical_owner: member.logical_owner.clone(),
            logical_runtime: EvidenceId::new(
                meerkat_runtime::LogicalRuntimeId::for_session(registration.session_id()).to_string(),
            ).map_err(|_| PrivateOriginalError::Unavailable)?,
            context: EvidenceId::new(registration.session_id().to_string())
                .map_err(|_| PrivateOriginalError::Unavailable)?,
            context_generation: member.context_generation,
            audience: member.audience.clone(),
        };
        let association = match record.final_association {
            Some(saved) => {
                if saved.candidate().authority_basis != selected_basis
                    || saved.candidate().target != target
                {
                    return Err(PrivateOriginalError::Unavailable);
                }
                saved
            }
            None => {
                let parent = route.controller_parent.clone()
                    .filter(|parent| !parent.is_empty()
                        && parent.len() < meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES)
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let facts = context.controller.plain_facts()
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                let mut bounds = application_operations::parent_restrictions(
                    application_operations::controller(
                        &native.grant_administrator, &route.logical_executor, &member.audience,
                        &facts.route_key(), application_operations::ControllerAction::Inference,
                    ),
                );
                bounds.delegation_depth = DelegationDepth::remaining(0);
                let mut candidate = InputAuthorityAssociationCandidate {
                    requester: record.source.requester.clone(),
                    ingress_actor: record.source.ingress_actor.clone(),
                    represented_subject: record.source.represented_subject.clone(),
                    original_authentication: super::governed_submit::evidence(
                        &record.source.authentication_domain, &record.event.original_authentication,
                    )?,
                    logical_executor: record.source.logical_executor.clone(),
                    target,
                    original_work: OriginalWorkRef {
                        authority: record.source.source_authority.clone(), work: record.event.original.clone(),
                    },
                    root_event: super::governed_submit::evidence(
                        &record.source.event_domain, &record.event.root_event,
                    )?,
                    contributing_work: record.event.contributors.iter().map(|work| OriginalWorkRef {
                        authority: record.source.source_authority.clone(), work: work.clone(),
                    }).collect(),
                    authority_basis: selected_basis,
                    controller_grant_lineage: parent.clone(),
                    controller_model: Some(context.controller.selection().clone()),
                    controller_ceiling: bounds.clone(),
                    admitted_ceiling: ExecutionRestrictions::unrestricted(),
                    source_observations: vec![],
                    ingress_namespace: QualifiedIngressNamespace {
                        realm: record.source.realm.clone(), ingress: record.source.ingress.clone(),
                        occurrence_scope: record.event.occurrence.clone(),
                    },
                    contract: ContractRequirements::local_governed_v1(),
                };
                // Validate the complete immutable source shape before native
                // issuance. Its controller parent is not yet admission-ready.
                InputAuthorityAssociation::new(candidate.clone())
                    .map_err(|_| PrivateOriginalError::Conflict)?;
                let grants = self.owner.grants();
                let caller = native.grant_administrator.clone();
                let executor = route.logical_executor.clone();
                let represented = route.represented_subject.clone();
                let child_id = EvidenceId::new(format!("original-controller-{}", self.key(reference)?.id))
                    .map_err(|_| PrivateOriginalError::Conflict)?;
                let issuance_lifetime = lifetime.clone();
                let issuance_console = console.clone();
                let issuance_login = approved_login.clone();
                let issued = tokio::task::spawn_blocking(move || {
                    // The blocking writer retains the same A or approved B
                    // login if its waiting binder disappears.
                    with_controller_login(issuance_console.as_ref(), issuance_login.as_ref(), || {
                    let issue = || {
                        let resolved = grants.resolve_controller_lineage(&parent, &caller, represented.as_ref())?;
                        let expected = resolved.restrictions().for_child(&bounds)
                            .map_err(|_| GrantRefusal::Denied)?;
                        resolved.check_current().map_err(|_| GrantRefusal::Unavailable)?;
                        // Identity lookup does not prove permission or intent.
                        // An unavailable lookup never becomes absence or issuance.
                        let child = match grants.issued_reference(&child_id)? {
                            Some(issued) => issued,
                            None => grants.issue_child(
                                &caller, parent.last().ok_or(GrantRefusal::Denied)?,
                                child_id, executor.clone(), bounds,
                            )?,
                        };
                        let mut lineage = parent;
                        lineage.push(child);
                        let resolved = grants.resolve_controller_lineage(&lineage, &executor, represented.as_ref())?;
                        if resolved.restrictions() != &expected { return Err(GrantRefusal::Denied); }
                        resolved.check_current().map_err(|_| GrantRefusal::Unavailable)?;
                        Ok::<_, GrantRefusal>(lineage)
                    };
                    issuance_lifetime.with_current_setup(|| issue()
                        .map_err(|_| HostLifetimeError::Unavailable))
                    })
                }).await.map_err(|_| PrivateOriginalError::Unavailable)?
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                // A lost issuance/publication response never selects a fresh
                // child ID. Only a currently usable exact parent/subject/bounds
                // join can recover the native identity above. Revoked, expired
                // or uncertain issuance stays unavailable for reconciliation.
                candidate.controller_grant_lineage = issued;
                InputAuthorityAssociation::new(candidate).map_err(|_| PrivateOriginalError::Conflict)?
            }
        };
        let claims = association.candidate();
        let custody = AuthenticatedOriginal {
            original_work: claims.original_work.clone(),
            occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
            original_authentication: claims.original_authentication.clone(),
            root_event: claims.root_event.clone(), contributing_work: claims.contributing_work.clone(),
            target: claims.target.clone(), authority_basis: claims.authority_basis.clone(),
        };
        if let Some(entry) = &console {
            entry.check().map_err(|_| PrivateOriginalError::Denied)?;
        }
        if let Some(login) = &approved_login {
            login.check().map_err(|_| PrivateOriginalError::Denied)?;
        }
        let original = match grant {
            None if batch.is_some() => self.bind_batch_source(reference, batch.unwrap(), association, custody).await?,
            Some(grant) => self.bind_access_source(reference, grant, association, custody).await?,
            None if selected_setup.is_some() => {
                let original = self.owner.bind_issued_access_async(
                    association, custody, requester, actor, Some(&native.grant_administrator),
                    |association| async move {
                        self.persist_or_observe_binding(reference, &association, true).await
                    },
                ).await.map_err(|error| match error {
                    OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
                    OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
                    _ => PrivateOriginalError::Unavailable,
                })?;
                self.hold_binding(reference.clone(), original.clone()).await?;
                original
            }
            None => {
                let original = self.owner.bind_controller_only_async(
                    association, custody, requester, actor,
                    |association| async move { self.persist_or_observe_binding(reference, &association, true).await },
                ).await.map_err(|error| match error {
                    OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
                    OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
                    _ => PrivateOriginalError::Unavailable,
                })?;
                self.hold_binding(reference.clone(), original.clone()).await?;
                original
            }
        };
        self.retained.lock().map_err(|_| PrivateOriginalError::Unavailable)?
            .iter().find(|held| Arc::ptr_eq(&held.original, &original)).cloned()
            .ok_or(PrivateOriginalError::Unavailable)
    }
}


/// Runs inside the actual blocking grant writer. The same owner decides entry
/// versus retirement; no caller-local guard or login mutex spans the work.
pub(super) fn with_controller_login<T>(
    console: Option<&super::console_ingress::ConsoleEntry>,
    approved_login: Option<&crate::private_host::ConsoleLoginHandle>,
    write: impl FnOnce() -> Result<T, HostLifetimeError>,
) -> Result<T, HostLifetimeError> {
    let reservation = match (console, approved_login) {
        (Some(entry), None) => Some(entry.reserve().map_err(|_| HostLifetimeError::Denied)?),
        (None, Some(login)) => Some(login.reserve().map_err(|_| HostLifetimeError::Denied)?),
        (None, None) => None,
        (Some(_), Some(_)) => return Err(HostLifetimeError::Denied),
    };
    let result = write();
    if result.is_ok() {
        if let Some(reservation) = reservation { reservation.settled(); }
    }
    result
}

#[cfg(test)]
#[path = "controller_login_tests.rs"]
mod controller_login_tests;
