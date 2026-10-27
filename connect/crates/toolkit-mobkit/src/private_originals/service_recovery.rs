//! Bounded read-only projection of existing operator-parent and access intent.
//! No source admission, new decision record, or automatic cleanup occurs.
use super::service_grants::recovery::{
    RetainedServiceSelection, ServiceRecoveryChild, ServiceRecoveryChildOwner,
};
use super::source_status::same_source_human;
use super::*;
use crate::installed_host::operator_setup::service_recovery::RetainedServiceApproval;
use meerkat_authorization::publication::LocalPublicationStamp;
use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_core::MobMemberBinding;

pub(crate) struct ServiceRecoveryRead {
    pub human: PrincipalRef,
    pub publication: LocalPublicationStamp,
    pub rows: Vec<ServiceRecoveryRow>,
}
pub(crate) struct ServiceRecoveryRow {
    pub id: String,
    pub projection: serde_json::Value,
    pub member: MobMemberBinding,
    pub can_recover: bool,
    pub(super) approval: RetainedServiceApproval,
    pub(super) prior: StoredRecord,
    pub(super) fresh_key: StoreKey,
    pub(super) fresh: Option<StoredRecord>,
    pub(super) published_member: bool,
    pub(super) selection: RetainedServiceSelection,
    pub(super) parent: Option<GrantLineageRef>,
    pub(super) child: Option<ServiceRecoveryChild>,
}

pub(crate) struct PreparedServiceRecovery<'a> {
    source: &'a PrivateOriginals,
    read: &'a ServiceRecoveryRead,
    row: &'a ServiceRecoveryRow,
    setup: Option<super::configured_setup::service::ServiceSetupCleanup>,
    _writer: tokio::sync::MutexGuard<'a, ()>,
}

#[derive(Serialize)]
pub(crate) struct ServiceRecoveryOutcome {
    pub parent: &'static str,
    pub child: &'static str,
    pub uncertain: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_children: Option<&'static str>,
}

fn revoke_outcome(
    result: Result<(), meerkat_authorization::grants::GrantRefusal>,
) -> (&'static str, bool) {
    use meerkat_authorization::grants::GrantRefusal;
    match result {
        Ok(()) => ("revoked", false),
        Err(GrantRefusal::ControllerInUse) => ("controller_in_use", false),
        Err(GrantRefusal::Unavailable) => ("unavailable", true),
        Err(GrantRefusal::Denied | GrantRefusal::Duplicate) => ("refused", false),
    }
}

impl PreparedServiceRecovery<'_> {
    /// The protected operator action supplies fresh entry authority. This is
    /// synchronous after entry: no dropped future can hide a parent mutation.
    pub(crate) fn revoke(self) -> Result<ServiceRecoveryOutcome, PrivateOriginalError> {
        use meerkat_authorization::grants::ExpectedRevokeRefusal;
        let parent = self
            .row
            .parent
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let child = self
            .row
            .child
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let machine = self.source.machine()?;
        let mut custody = machine
            .try_controller_grant_mutation()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let parent_result = self.source.owner.grants().revoke_expecting(
            &self.read.human,
            parent,
            &self.read.publication,
            &mut custody,
        );
        let refused = match parent_result {
            Ok(()) => None,
            Err(ExpectedRevokeRefusal::NotCurrent) => Some(("not_current", false)),
            Err(ExpectedRevokeRefusal::Grant(error)) => Some(revoke_outcome(Err(error))),
        };
        if let Some((parent, uncertain)) = refused {
            return Ok(ServiceRecoveryOutcome {
                parent,
                child: "not_entered",
                uncertain,
                setup_children: self.setup.as_ref().map(|_| "not_entered"),
            });
        }
        // Revoked parent prevents any later issuance under this exact parent.
        // The canonical child owner was captured before native entry.
        let mut uncertain = false;
        let child_state = match &child.owner {
            ServiceRecoveryChildOwner::AnotherApproval => "another_approval_preserved",
            selected => match self
                .source
                .service_recovery_child_reference(&self.row.selection)
            {
                Ok(None) => "not_issued",
                Ok(Some(leaf)) => match selected {
                    ServiceRecoveryChildOwner::ThisApproval(issuer) => {
                        let (state, unknown) = revoke_outcome(self.source.owner.grants().revoke(
                            issuer,
                            &leaf,
                            &mut custody,
                        ));
                        uncertain |= unknown;
                        state
                    }
                    _ => {
                        uncertain = true;
                        "unresolved"
                    }
                },
                Err(_) => {
                    uncertain = true;
                    "unavailable"
                }
            },
        };
        drop(custody);
        let setup_children = self.setup.map(|setup| match setup.revoke(self.source) {
            Ok(()) => "revoked_or_not_issued",
            Err(_) => { uncertain = true; "unresolved" },
        });
        Ok(ServiceRecoveryOutcome {
            parent: "revoked",
            child: child_state,
            uncertain,
            setup_children,
        })
    }
}
impl ServiceRecoveryRead {
    pub(crate) fn check_visible(
        &self,
        permits: impl Fn(&MobMemberBinding) -> bool,
    ) -> Result<(), PrivateOriginalError> {
        self.publication
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if self
            .rows
            .iter()
            .any(|row| row.approval.cleanup.issuer() != &self.human || !permits(&row.member))
        {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(())
    }
}

impl PrivateOriginals {
    pub(crate) async fn prepare_service_recovery<'a>(
        &'a self,
        read: &'a ServiceRecoveryRead,
        row: &'a ServiceRecoveryRow,
    ) -> Result<PreparedServiceRecovery<'a>, PrivateOriginalError> {
        let writer = self
            .owner
            .try_source_writer()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        self.recheck_service_recovery(read, row).await?;
        if !row.can_recover || row.published_member || row.parent.is_none() || row.child.is_none() {
            return Err(PrivateOriginalError::Denied);
        }
        let setup = if matches!(row.child.as_ref().map(|child| &child.owner),
            Some(ServiceRecoveryChildOwner::ThisApproval(_))) {
            if let Some(fresh) = &row.fresh {
                let record = self.decode_stored(fresh)?;
                match record.approved_task.as_ref() {
                    Some(task) if task.setup().is_some() && task.contains(&row.selection.reference) => {
                        let reference = PrivateOriginalRef { route: record.event.route.clone(),
                            original: record.event.original.clone(),
                            event_sha256: digest(&canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?) };
                        Some(self.prepare_service_setup_cleanup(&reference, fresh).await?)
                    },
                    _ => None,
                }
            } else { None }
        } else { None };
        // No admission/current-task test: the authenticated operator can clean
        // up a cancelled or cold original without reviving its authority.
        Ok(PreparedServiceRecovery {
            source: self,
            read,
            row,
            setup,
            _writer: writer,
        })
    }

    pub(crate) async fn read_service_recovery_page(
        &self,
        human: &PrincipalRef,
        members: &[MobMemberBinding],
        rows: &[StoredRecord],
    ) -> Result<ServiceRecoveryRead, PrivateOriginalError> {
        let lifetime = self.status_lifetime()?;
        let operator = self
            .service_operator_setup(lifetime.clone())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if human.kind != meerkat_core::PrincipalKind::Human
            || human.validate_qualified().is_err()
            || operator.recovery_operator() != human
        {
            return Err(PrivateOriginalError::Denied);
        }
        let (_, publication) = self
            .owner
            .publication()
            .observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut read = ServiceRecoveryRead {
            human: human.clone(),
            publication,
            rows: Vec::new(),
        };
        for stored in rows {
            if stored.key.scope() != self.scope {
                return Err(PrivateOriginalError::Conflict);
            }
            let Some(approval) = operator
                .retained_service_approval(stored)
                .map_err(|_| PrivateOriginalError::Unavailable)?
            else {
                continue;
            };
            if approval.cleanup.issuer() != human {
                continue;
            }
            let context = &approval.context;
            let prior_key = StoreKey {
                realm: self.scope.realm.clone(),
                owner: self.scope.owner.clone(),
                id: source_key_id(
                    &RealmId::parse(self.scope.realm.clone())
                        .map_err(|_| PrivateOriginalError::Conflict)?,
                    self.owner.ingress_domain(),
                    &context.prior_original.authority,
                    &context.prior_original.work,
                )?,
            };
            let prior = self
                .store
                .get(&prior_key)
                .await
                .map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let record = self.decode_stored(&prior)?;
            if !same_source_human(
                human,
                &record.source.requester,
                record.source.represented_subject.as_ref(),
            ) {
                continue;
            }
            let Some(route) = self.routes()?.iter().find(|route| {
                route.id == *approval.decision.route()
                    && same_source_human(
                        human,
                        &route.requester,
                        route.represented_subject.as_ref(),
                    )
            }) else {
                continue;
            };
            let member = self.status_route(&lifetime, human, route)?;
            if !members.contains(&member) {
                continue;
            }
            let prior_ref = PrivateOriginalRef {
                route: record.event.route.clone(),
                original: record.event.original.clone(),
                event_sha256: digest(
                    &canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?,
                ),
            };
            let record = self.decode_record(&prior, &prior_ref)?;
            let association = record
                .final_association
                .as_ref()
                .ok_or(PrivateOriginalError::Unavailable)?;
            let claims = association.candidate();
            let scope = meerkat_toolkit::setup::SetupScope::new(
                Id::new(self.scope.realm.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
                Id::new(self.scope.owner.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
            );
            if prior_ref.route != *approval.decision.route()
                || claims.original_work != context.prior_original
                || claims.target != context.live_attachment.destination
                || record.source.requester != context.operator_and_requester
                || record.source.ingress_actor != context.ingress_actor
                || record.source.represented_subject != context.represented_subject
                || context.audience != route.grant_audience
                || approval.application != route.grant_caller
                || approval.represented_subject != route.represented_subject
                || scope.key(
                    &Id::new(context.business_task.id.clone())
                        .map_err(|_| PrivateOriginalError::Conflict)?,
                ) != context.business_task
            {
                return Err(PrivateOriginalError::Conflict);
            }
            let selection = match (&approval.decision, &approval.selection) {
                (super::service_grants::AccessDecision::Mcp(decision), Some(selection)) =>
                    self.retained_service_selection(decision, selection, &approval.requested)?,
                (super::service_grants::AccessDecision::Catalog(decision), None) =>
                    self.retained_catalog_selection(decision, &approval.requested)?,
                _ => return Err(PrivateOriginalError::Conflict),
            };
            let fresh_key = StoreKey {
                realm: self.scope.realm.clone(),
                owner: self.scope.owner.clone(),
                id: source_key_id(
                    &record.source.realm,
                    &record.source.ingress,
                    &record.source.source_authority,
                    &context.fresh_original,
                )?,
            };
            let fresh = self.store.get(&fresh_key).await.map_err(store_error)?;
            if let Some(fresh) = &fresh {
                let published = self.decode_stored(fresh)?;
                if published.source != record.source
                    || published.event.original != context.fresh_original
                    || published.approved_task.as_ref().is_none_or(|link| {
                        link.parent != prior_ref || link.acquisition != context.business_task
                    })
                {
                    return Err(PrivateOriginalError::Conflict);
                }
            }
            // A missing reference remains unresolved: a warm parent issuance
            // may not have entered yet. It is never a cleanup/no-effect result.
            let observed = approval.cleanup.reference_observation().await;
            let parent = observed.as_ref().ok().and_then(Clone::clone);
            let child_result = match &parent {
                Some(parent) => Some(self.service_recovery_child(&selection, parent).await),
                None => None,
            };
            let published_member = match (&fresh, &parent) {
                (Some(fresh), Some(parent)) => {
                    let published = self.decode_stored(fresh)?;
                    let link = published.approved_task.as_ref().ok_or(PrivateOriginalError::Conflict)?;
                    let selected = link.contains(&selection.reference)
                        && self.service_compensation_child(&selection.reference, parent).await?.is_some();
                    // A cancelled, provably unbound batch can clean its own
                    // selected partial effects. Legacy publication stays protected.
                    let rejected_batch = matches!(link, super::service_approved_input::ApprovedTaskLink::Batch(_))
                        && published.operation_cancelled && published.final_association.is_none()
                        && published.service_admission.is_none()
                        && published.bound_service_input.is_none();
                    selected && !rejected_batch
                }
                (Some(_), None) => true,
                _ => false,
            };
            let can_recover = !published_member
                && parent.is_some()
                && child_result.as_ref().is_some_and(Result::is_ok);
            let parent_state = match (&observed, &parent) {
                (Err(_), _) => "unavailable",
                (_, None) => "not_observed",
                (_, Some(parent)) => match self.owner.grants().resolve_lineage(
                    &[parent.clone()],
                    &approval.application,
                    approval.represented_subject.as_ref(),
                ) {
                    Ok(resolved)
                        if resolved.check_current().is_ok()
                            && resolved.restrictions() == &approval.requested =>
                    {
                        "issued"
                    }
                    _ => "unavailable",
                },
            };
            let child_state = match &child_result {
                Some(Ok(child)) => child.state,
                Some(Err(_)) => "unavailable",
                None => "not_observed",
            };
            let policy = match &approval.policy {
                None => "not_requested",
                Some(policy) => match self.service_tool_policy.get() {
                    Some(owner) => owner
                        .recovery_observation(policy, &prior_ref, &context.business_task)
                        .await
                        .unwrap_or("unavailable"),
                    None => "unavailable",
                },
            };
            let id = digest(
                &canonical_json(&(
                    human,
                    &stored.key,
                    &stored.content_digest,
                    &prior.content_digest,
                ))
                .map_err(|_| PrivateOriginalError::Conflict)?,
            );
            let mut projection = selection.projection();
            projection["id"] = serde_json::json!(id);
            projection["state"] = serde_json::json!(if published_member {
                "publication_present"
            } else {
                "publication_unresolved"
            });
            projection["parent"] = serde_json::json!(parent_state);
            projection["child"] = serde_json::json!(child_state);
            projection["policy"] = serde_json::json!(policy);
            projection["can_recover"] = serde_json::json!(can_recover);
            read.rows.push(ServiceRecoveryRow {
                id,
                projection,
                member,
                can_recover,
                approval,
                prior,
                fresh_key,
                fresh,
                published_member,
                selection,
                parent,
                child: child_result.and_then(Result::ok),
            });
        }
        for row in &read.rows {
            self.recheck_service_recovery(&read, row).await?;
        }
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Denied)?;
        read.publication
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(read)
    }

    pub(crate) async fn recheck_service_recovery(
        &self,
        read: &ServiceRecoveryRead,
        row: &ServiceRecoveryRow,
    ) -> Result<(), PrivateOriginalError> {
        if row.approval.cleanup.issuer() != &read.human
            || !row.approval.cleanup.belongs_to(&self.owner)
        {
            return Err(PrivateOriginalError::Denied);
        }
        for stored in [&row.approval.stored, &row.prior] {
            if self
                .store
                .get(&stored.key)
                .await
                .map_err(store_error)?
                .as_ref()
                != Some(stored)
            {
                return Err(PrivateOriginalError::Unavailable);
            }
        }
        if self.store.get(&row.fresh_key).await.map_err(store_error)? != row.fresh {
            return Err(PrivateOriginalError::Unavailable);
        }
        if let Some(child) = &row.child {
            let actual = self
                .service_recovery_child(
                    &row.selection,
                    row.parent.as_ref().ok_or(PrivateOriginalError::Conflict)?,
                )
                .await?;
            if actual.stored != child.stored
                || actual.native != child.native
                || actual.state != child.state
            {
                return Err(PrivateOriginalError::Unavailable);
            }
        }
        read.publication
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)
    }
}
