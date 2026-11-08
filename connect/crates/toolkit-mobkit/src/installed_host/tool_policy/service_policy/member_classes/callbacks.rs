//! Callback grants live in the existing native grant owner. The canonical
//! immutable admission is their issuance intent; deterministic IDs only locate
//! that owner's actual records. They never prove issuance or currentness.
use super::*;
use crate::installed_host::application_operations;
use meerkat_authorization_contracts::{
    constraints::{ActionRef, DelegationDepth, ExactRestriction},
    evidence::EvidenceId,
    work_association::{GrantTarget, TargetGrantLineage},
};

fn root_id(bundle: &str) -> Result<EvidenceId, HostLifetimeError> {
    EvidenceId::new(format!("{bundle}/grant")).map_err(|_| invalid())
}
fn leaf_id(bundle: &str, operation: &str) -> Result<EvidenceId, HostLifetimeError> {
    let bytes = canonical_json(&(bundle, operation)).map_err(|_| invalid())?;
    EvidenceId::new(format!("toolkit.class-operation/{}", sha(&bytes))).map_err(|_| invalid())
}

impl ServicePolicyOwner {
    pub(crate) fn bind_callback_source(
        &self,
        source: &Arc<crate::private_originals::PrivateOriginals>,
    ) -> Result<(), HostLifetimeError> {
        let selected = self
            .callback_source
            .get_or_init(|| Arc::downgrade(source))
            .upgrade()
            .ok_or_else(unavailable)?;
        if Arc::ptr_eq(&selected, source) {
            Ok(())
        } else {
            Err(HostLifetimeError::WrongOwner)
        }
    }

    /// Source writer precedes this existing policy exclusion. All native grant
    /// operations below are synchronous and never acquire the source writer.
    pub(crate) async fn callback_targets(
        &self,
        expected: &MemberClassAdmission,
        route: &meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
        issue: bool,
    ) -> Result<Vec<TargetGrantLineage>, HostLifetimeError> {
        let _exclusion = self.exclusion.lock().await;
        let current = self
            .observed_admission(&expected.instance.member.member)?
            .ok_or(HostLifetimeError::Denied)?;
        if current != *expected
            || current.route.as_ref() != Some(route)
            || !matches!(current.phase, MemberClassAdmissionPhase::Active { .. })
        {
            return Err(HostLifetimeError::Denied);
        }
        let operations = current
            .class
            .callbacks
            .iter()
            .filter(|callback| {
                current.class.grants.iter().any(|grant| {
                    grant.tool_name == callback.name
                        && grant.action == meerkat_core::CompiledMemberToolAction::Invoke
                })
            })
            .map(|callback| callback.name.clone())
            .collect::<Vec<_>>();
        if operations.is_empty() {
            return Ok(vec![]);
        }
        if operations.len()
            > meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let source = self
            .callback_source
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(unavailable)?;
        let context = source.callback_grant_context()?;
        let targets = context.lifetime.with_current_setup(|| {
            let bundle = self.callback_bundle(&current)?;
            let values = application_operations::class_callback(
                &context.application,
                &route.logical_executor,
                &current.instance.member.audience,
                &bundle,
                &operations[0],
            );
            let mut root_bounds = application_operations::parent_restrictions(values);
            root_bounds.actions =
                ExactRestriction::exact(operations.iter().map(|operation| ActionRef {
                    feature: "toolkit-class-callback".into(),
                    action: operation.clone(),
                }));
            let id = root_id(&bundle)?;
            let (root, new_root) = match context
                .grants
                .issued_reference(&id)
                .map_err(|_| unavailable())?
            {
                Some(root) => (root, false),
                None if issue => (
                    context
                        .grants
                        .issue_root(
                            &context.operator,
                            id,
                            context.application.clone(),
                            route.represented_subject.clone(),
                            root_bounds.clone(),
                        )
                        .map_err(|_| unavailable())?,
                    true,
                ),
                None => return Err(unavailable()),
            };
            // Reconstruction compares the exact native record. Target currentness
            // is intentionally left to its own operation on replay.
            if root.root_authority != context.operator {
                return Err(HostLifetimeError::Denied);
            }
            check_issuance(
                &context.grants,
                std::slice::from_ref(&root),
                &context.application,
                route.represented_subject.as_ref(),
                &root_bounds,
                new_root,
            )?;
            let mut targets = Vec::with_capacity(operations.len());
            for operation in operations {
                let mut bounds = application_operations::parent_restrictions(
                    application_operations::class_callback(
                        &context.application,
                        &route.logical_executor,
                        &current.instance.member.audience,
                        &bundle,
                        &operation,
                    ),
                );
                bounds.delegation_depth = DelegationDepth::remaining(0);
                let id = leaf_id(&bundle, &operation)?;
                let (leaf, new_leaf) = match context
                    .grants
                    .issued_reference(&id)
                    .map_err(|_| unavailable())?
                {
                    Some(leaf) => (leaf, false),
                    None if issue => (
                        context
                            .grants
                            .issue_child(
                                &context.application,
                                &root,
                                id,
                                route.logical_executor.clone(),
                                bounds.clone(),
                            )
                            .map_err(|_| unavailable())?,
                        true,
                    ),
                    None => return Err(unavailable()),
                };
                let lineage = vec![root.clone(), leaf];
                check_issuance(
                    &context.grants,
                    &lineage,
                    &route.logical_executor,
                    route.represented_subject.as_ref(),
                    &bounds,
                    new_leaf,
                )?;
                targets.push(TargetGrantLineage {
                    target: GrantTarget::RegisteredToolBundleOperation {
                        bundle: bundle.clone(),
                        operation,
                    },
                    lineage,
                });
            }
            targets.sort_by(|a, b| a.target.cmp(&b.target));
            Ok(targets)
        })?;
        // The application reservation can invoke host code. Recheck it only
        // after leaving the bounded native mutation fence.
        self.observed_admission(&expected.instance.member.member)?
            .filter(|now| now == expected)
            .ok_or(HostLifetimeError::Denied)?;
        Ok(targets)
    }

    /// Historical source rebind checks exact native identities without
    /// requiring unrelated targets to remain usable. Native revocation keeps
    /// these references; each operation resolves its own current lineage.
    pub(crate) fn check_callback_references(
        &self,
        admission: &MemberClassAdmission,
        targets: &[TargetGrantLineage],
    ) -> Result<(), HostLifetimeError> {
        if targets.is_empty() {
            return Ok(());
        }
        let source = self
            .callback_source
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(unavailable)?;
        let context = source.callback_grant_context()?;
        let bundle = self.callback_bundle(admission)?;
        let root = context
            .grants
            .issued_reference(&root_id(&bundle)?)
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        if root.root_authority != context.operator {
            return Err(HostLifetimeError::Denied);
        }
        for row in targets {
            let GrantTarget::RegisteredToolBundleOperation {
                bundle: selected,
                operation,
            } = &row.target
            else {
                return Err(HostLifetimeError::Denied);
            };
            if selected != &bundle {
                return Err(HostLifetimeError::Denied);
            }
            let leaf = context
                .grants
                .issued_reference(&leaf_id(&bundle, operation)?)
                .map_err(|_| unavailable())?
                .ok_or_else(unavailable)?;
            if row.lineage != [root.clone(), leaf] {
                return Err(HostLifetimeError::Denied);
            }
        }
        Ok(())
    }

    pub(super) fn revoke_class_callbacks(
        &self,
        admission: &MemberClassAdmission,
    ) -> Result<(), HostLifetimeError> {
        if let Some(standing) = &admission.standing_operations {
            let source = self
                .callback_source
                .get()
                .and_then(std::sync::Weak::upgrade)
                .ok_or_else(unavailable)?;
            // Intent survives a failed second issue/commit. Revoke whichever
            // exact parents exist, even when no complete pair was published.
            source.revoke_callback_root(&standing.peer_message_parent)?;
            source.revoke_callback_root(&standing.recorder_write_parent)?;
        }
        // Issuance requires an Active admission with this exact retained
        // route and a declared Invoke callback. Only those existing facts can
        // prove nonissuance; an absent process pointer cannot do so on reopen.
        if admission.route.is_none()
            || !admission.class.callbacks.iter().any(|callback| {
                admission.class.grants.iter().any(|grant| {
                    grant.tool_name == callback.name
                        && grant.action == meerkat_core::CompiledMemberToolAction::Invoke
                })
            })
        {
            return Ok(());
        }
        let source = self
            .callback_source
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(unavailable)?;
        source.revoke_callback_root(&root_id(&self.callback_bundle(admission)?)?)
    }
}

/// Reusing an existing target does not require that unrelated target to remain
/// usable. A denied retained lineage stays selected as denied; it is never
/// replaced. Native custody uncertainty is different and refuses composition.
fn check_issuance(
    grants: &meerkat_authorization::grants::LocalGrantAuthority,
    lineage: &[meerkat_authorization_contracts::work_association::GrantLineageRef],
    executor: &meerkat_core::PrincipalRef,
    represented: Option<&meerkat_core::PrincipalRef>,
    expected: &meerkat_authorization_contracts::constraints::ExecutionRestrictions,
    newly_issued: bool,
) -> Result<(), HostLifetimeError> {
    match grants.resolve_lineage(lineage, executor, represented) {
        Ok(resolved) if resolved.restrictions() == expected => Ok(()),
        Ok(_) => Err(HostLifetimeError::Denied),
        Err(meerkat_authorization::grants::GrantRefusal::Denied) if !newly_issued => Ok(()),
        Err(meerkat_authorization::grants::GrantRefusal::Denied) => Err(HostLifetimeError::Denied),
        Err(_) => Err(HostLifetimeError::Unavailable),
    }
}
