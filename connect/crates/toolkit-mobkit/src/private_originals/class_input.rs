//! Ordinary class inputs retain their own exact target selection. They are
//! neither configured acquisition helpers nor versioned reviewed service sets.
use super::member_mcp::MemberMcpSourceSelection;
use super::*;
use crate::installed_host::{application_operations, dynamic_members::MemberClassAdmission};
use meerkat_authorization_contracts::{
    evidence::EvidenceId,
    work_association::{
        GrantTarget, MAX_ASSOCIATION_REFERENCES, TargetGrantLineage, WorkAuthorityBasis,
    },
};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClassInputSelection {
    admission: Id,
    bundle: String,
    targets: Vec<TargetGrantLineage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp: Option<MemberMcpSourceSelection>,
}

/// The one composition seam for independently owner-checked MCP targets.
/// Callers must supply actual selected lineages, never names converted to grants.
pub(crate) fn compose_targets(
    mut callbacks: Vec<TargetGrantLineage>,
    mcp: Vec<TargetGrantLineage>,
) -> Result<WorkAuthorityBasis, HostLifetimeError> {
    if callbacks.iter().any(|row| {
        !matches!(
            row.target,
            GrantTarget::RegisteredToolBundleOperation { .. }
        )
    }) || mcp
        .iter()
        .any(|row| !matches!(row.target, GrantTarget::McpServer { .. }))
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    callbacks.extend(mcp);
    callbacks.sort_by(|a, b| a.target.cmp(&b.target));
    if callbacks.len() > MAX_ASSOCIATION_REFERENCES
        || callbacks
            .windows(2)
            .any(|pair| pair[0].target >= pair[1].target)
        || callbacks.iter().any(|row| row.lineage.is_empty())
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    Ok(WorkAuthorityBasis::TargetedGrantLineages {
        base: None,
        targets: callbacks,
    })
}

pub(crate) struct CallbackGrantContext {
    pub grants: Arc<meerkat_authorization::grants::LocalGrantAuthority>,
    pub lifetime: Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
    pub operator: PrincipalRef,
    pub application: PrincipalRef,
}

impl PrivateOriginals {
    pub(crate) fn callback_grant_context(&self) -> Result<CallbackGrantContext, HostLifetimeError> {
        let context = self.retained_callback_grant_context()?;
        context.lifetime.validate_current_selection()?;
        Ok(context)
    }

    // Cleanup retains the immutable issuer and the same native grant authority.
    // It cannot require Open: close fences issuance before joining accepted
    // operations, whose exact grants still need narrowing during that join.
    fn retained_callback_grant_context(&self) -> Result<CallbackGrantContext, HostLifetimeError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        let native = lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        Ok(CallbackGrantContext {
            grants: self.owner.grants(),
            operator: native
                .remote_operator
                .clone()
                .ok_or(HostLifetimeError::Denied)?,
            application: native.grant_administrator.clone(),
            lifetime,
        })
    }

    pub(crate) fn revoke_callback_root(&self, id: &EvidenceId) -> Result<(), HostLifetimeError> {
        let context = self.retained_callback_grant_context()?;
        let Some(root) = context
            .grants
            .issued_reference(id)
            .map_err(|_| HostLifetimeError::Unavailable)?
        else {
            return Ok(());
        };
        if root.root_authority != context.operator {
            return Err(HostLifetimeError::WrongOwner);
        }
        // This is only revocation of the canonical admission's exact root.
        // Native mutation custody owns its publication and active-use checks;
        // no new input, grant or callback is admitted through this cleanup path.
        let machine = self.machine().map_err(|_| HostLifetimeError::Unavailable)?;
        let mut custody = machine
            .try_controller_grant_mutation()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        context
            .grants
            .revoke(&context.operator, &root, &mut custody)
            .map_err(|_| HostLifetimeError::Unavailable)
    }

    fn selected_class_input(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<
        Option<(
            MemberClassAdmission,
            meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
        )>,
        PrivateOriginalError,
    > {
        let Some(owner) = self.service_tool_policy.get() else {
            return Ok(None);
        };
        owner
            .member_route(&reference.route)
            .map_err(|_| PrivateOriginalError::Unavailable)
    }

    pub(super) async fn class_input_basis(
        &self,
        record: &SourceRecord,
        reference: &PrivateOriginalRef,
        session: &meerkat_core::SessionId,
    ) -> Result<Option<WorkAuthorityBasis>, PrivateOriginalError> {
        if record.setup_request.is_some()
            || record.configured_setup.is_some()
            || record.approved_task.is_some()
            || record.service_admission.is_some()
        {
            if record.class_input.is_some() {
                return Err(PrivateOriginalError::Conflict);
            }
            return Ok(None);
        }
        // Existing inputs are immutable. A newly introduced class policy does
        // not upgrade an older controller-only original on retry.
        if record.final_association.is_some() && record.class_input.is_none() {
            return Ok(None);
        }
        let Some((observed, observed_route)) = self.selected_class_input(reference)? else {
            return if record.class_input.is_none() {
                Ok(None)
            } else {
                Err(PrivateOriginalError::Denied)
            };
        };
        if !observed.class.purpose.is_ordinary() || observed.workflow.is_some() {
            return Err(PrivateOriginalError::Denied);
        }
        if record.operation_cancelled
            || observed
                .destination
                .as_ref()
                .is_none_or(|destination| destination.context.as_str() != session.to_string())
        {
            return Err(PrivateOriginalError::Denied);
        }
        // The application's retained row lookup can await its private pipe.
        // Never hold the source writer across that external participant.
        let mcp = self
            .select_member_mcp_source(
                &observed,
                record
                    .class_input
                    .as_ref()
                    .and_then(|saved| saved.mcp.as_ref()),
            )
            .await
            .map_err(|error| match error {
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
                _ => PrivateOriginalError::Denied,
            })?;
        // Even an explicit empty selection is a retained application fact.
        // A historical None cannot be silently upgraded on replay.
        if mcp.is_none()
            || record
                .class_input
                .as_ref()
                .is_some_and(|saved| saved.mcp != mcp)
        {
            return Err(PrivateOriginalError::Denied);
        }
        let _writer = self.owner.source_writer().await;
        let stored = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let mut current = self.decode_record(&stored, reference)?;
        let Some((admission, route)) = self.selected_class_input(reference)? else {
            return if current.class_input.is_none() {
                Ok(None)
            } else {
                Err(PrivateOriginalError::Denied)
            };
        };
        if admission != observed
            || route != observed_route
            || current.class_input != record.class_input
        {
            return Err(PrivateOriginalError::Conflict);
        }
        if admission
            .destination
            .as_ref()
            .is_none_or(|destination| destination.context.as_str() != session.to_string())
            || current.operation_cancelled
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.check_member_mcp_source(
            &admission,
            mcp.as_ref().ok_or(PrivateOriginalError::Denied)?,
        )
        .map_err(|error| match error {
            HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                PrivateOriginalError::Unavailable
            }
            _ => PrivateOriginalError::Denied,
        })?;
        if let Some(selected) = &current.class_input {
            self.check_class_selection(reference, selected)?;
            return Ok(Some(WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: selected.targets.clone(),
            }));
        }
        if current.final_association.is_some() {
            return Ok(None);
        }
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let callbacks = owner
            .callback_targets(&admission, &route, true)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = compose_targets(
            callbacks,
            mcp.as_ref()
                .map(|selected| selected.targets())
                .unwrap_or_default(),
        )
        .map_err(|_| PrivateOriginalError::Conflict)?
        else {
            unreachable!()
        };
        let selected = ClassInputSelection {
            admission: admission.id.clone(),
            bundle: owner
                .callback_bundle(&admission)
                .map_err(|_| PrivateOriginalError::Conflict)?,
            targets,
            mcp,
        };
        self.check_class_selection(reference, &selected)?;
        current.class_input = Some(selected.clone());
        self.store
            .compare_and_swap(Transition {
                key: stored.key.clone(),
                expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest.clone()),
                claim: None,
                data: encode(current).map_err(|_| PrivateOriginalError::Conflict)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(store_error)?;
        Ok(Some(WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: selected.targets,
        }))
    }

    fn check_class_selection(
        &self,
        reference: &PrivateOriginalRef,
        selected: &ClassInputSelection,
    ) -> Result<MemberClassAdmission, PrivateOriginalError> {
        let (admission, _) = self
            .selected_class_input(reference)?
            .ok_or(PrivateOriginalError::Denied)?;
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if admission.id != selected.admission
            || owner
                .callback_bundle(&admission)
                .map_err(|_| PrivateOriginalError::Conflict)?
                != selected.bundle
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let mut expected = admission
            .class
            .callbacks
            .iter()
            .filter(|callback| {
                admission.class.grants.iter().any(|grant| {
                    grant.tool_name == callback.name
                        && grant.action == meerkat_core::CompiledMemberToolAction::Invoke
                })
            })
            .map(|callback| GrantTarget::RegisteredToolBundleOperation {
                bundle: selected.bundle.clone(),
                operation: callback.name.clone(),
            })
            .collect::<Vec<_>>();
        expected.sort();
        let callbacks = selected
            .targets
            .iter()
            .filter(|row| {
                matches!(
                    row.target,
                    GrantTarget::RegisteredToolBundleOperation { .. }
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        if callbacks.iter().map(|row| &row.target).ne(expected.iter())
            || callbacks.iter().any(|row| row.lineage.len() != 2)
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let mcp = match &selected.mcp {
            Some(mcp) => {
                self.check_member_mcp_source(&admission, mcp)
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                mcp.targets()
            }
            None => vec![],
        };
        let expected_basis =
            compose_targets(callbacks.clone(), mcp).map_err(|_| PrivateOriginalError::Conflict)?;
        if expected_basis
            != (WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: selected.targets.clone(),
            })
        {
            return Err(PrivateOriginalError::Conflict);
        }
        owner
            .check_callback_references(&admission, &callbacks)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(admission)
    }

    pub(super) fn check_class_input_binding(
        &self,
        reference: &PrivateOriginalRef,
        record: &SourceRecord,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<bool, OriginalWriterError> {
        let Some(selected) = &record.class_input else {
            return Ok(false);
        };
        if record.setup_request.is_some()
            || record.configured_setup.is_some()
            || record.approved_task.is_some()
            || record.service_admission.is_some()
        {
            return Err(OriginalWriterError::Conflict);
        }
        self.check_class_selection(reference, selected)
            .map_err(|_| OriginalWriterError::Unavailable)?;
        if association.candidate().authority_basis
            != (WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: selected.targets.clone(),
            })
        {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(true)
    }

    pub(crate) fn class_callback_operation(
        &self,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
        bundle: &str,
        operation: &str,
        call_name: &str,
    ) -> Result<meerkat_authorization::policy::LocalOperationValues, PrivateOriginalError> {
        let route_id = self.route_for_binding(association)?;
        let owner = self
            .service_tool_policy
            .get()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let (admission, route) = owner
            .member_route(&route_id)
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .ok_or(PrivateOriginalError::Denied)?;
        if operation != call_name
            || owner
                .callback_bundle(&admission)
                .map_err(|_| PrivateOriginalError::Conflict)?
                != bundle
            || !admission
                .class
                .callbacks
                .iter()
                .any(|callback| callback.name == operation)
            || !admission.class.grants.iter().any(|grant| {
                grant.tool_name == operation
                    && grant.action == meerkat_core::CompiledMemberToolAction::Invoke
            })
            || association
                .candidate()
                .authority_basis
                .lineage_for(Some(&GrantTarget::RegisteredToolBundleOperation {
                    bundle: bundle.into(),
                    operation: operation.into(),
                }))
                .is_none()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let native = self
            .callback_grant_context()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(application_operations::class_callback(
            &native.application,
            &route.logical_executor,
            &admission.instance.member.audience,
            bundle,
            operation,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_authorization_contracts::grant::{GrantAuthorityIncarnation, GrantLineageRef};

    // Shape-only data for the pure combiner. This never claims native issuance.
    fn reference() -> GrantLineageRef {
        GrantLineageRef {
            root_authority: PrincipalRef::in_domain(
                meerkat_core::PrincipalKind::ServiceAccount,
                "fixture",
                meerkat_core::TrustDomainId::new("fixture").unwrap(),
            )
            .unwrap(),
            authority_namespace: EvidenceId::new("fixture").unwrap(),
            authority_generation: 1,
            authority_incarnation: GrantAuthorityIncarnation::from_uuid(uuid::Uuid::new_v4())
                .unwrap(),
            grant_id: EvidenceId::new("shape-only").unwrap(),
            issued_revision: 0,
        }
    }
    fn callback(operation: &str) -> TargetGrantLineage {
        TargetGrantLineage {
            target: GrantTarget::RegisteredToolBundleOperation {
                bundle: "fixture-class".into(),
                operation: operation.into(),
            },
            lineage: vec![reference()],
        }
    }
    fn mcp() -> TargetGrantLineage {
        TargetGrantLineage {
            target: GrantTarget::McpServer {
                server: "reviewed-server".into(),
            },
            lineage: vec![reference()],
        }
    }
    #[test]
    fn independent_targets_compose_canonically_without_a_base_or_silent_deduplication() {
        let left = callback("b");
        let right = callback("a");
        let server = mcp();
        let WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets,
        } = compose_targets(vec![left.clone(), right.clone()], vec![server.clone()]).unwrap()
        else {
            panic!("targeted only");
        };
        assert_eq!(targets.len(), 3);
        assert!(
            targets
                .windows(2)
                .all(|rows| rows[0].target < rows[1].target)
        );
        assert!(targets.contains(&left) && targets.contains(&right) && targets.contains(&server));
        assert!(compose_targets(vec![left.clone(), left], vec![]).is_err());
        assert!(compose_targets(vec![server], vec![]).is_err());
        assert!(compose_targets(vec![], vec![right]).is_err());
        let callbacks = (0..63)
            .map(|n| callback(&format!("operation-{n:02}")))
            .collect::<Vec<_>>();
        assert!(compose_targets(callbacks.clone(), vec![mcp()]).is_ok());
        let mut overflow = callbacks;
        overflow.push(callback("operation-63"));
        assert!(compose_targets(overflow, vec![mcp()]).is_err());
    }
}
