//! Canonical process-local grant issuance, revocation and full-lineage use.
//!
//! This host retains the generated owner directly. It is not a second grant
//! registry, a durable store or restoration authority. Administrative root
//! selection and authenticated callers come from the embedding's actual owners.
//! Every mutation shares the publication selected by the embedding.
//!
//! A resolved grant is only one conjunct: policy must independently authorize
//! the actual requester to invoke that delegation, the executing agent, source,
//! peer, credential/account and any represented subject. Holding a credential or
//! being an agent's human owner does not activate that human's permissions.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, LifetimeBound};
use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::grant::{GrantAuthorityIncarnation, GrantLineageRef};
use meerkat_core::auth::PrincipalRef;

use crate::clock::LocalAuthorizationClock;
use crate::publication::{LocalAuthorizationPublication, LocalPublicationStamp, PublicationError};

pub(crate) mod dsl;
mod reconstruction;
use dsl::{
    GrantAuthorityEffect, GrantAuthorityInput, GrantAuthorityMachineAuthority,
    GrantAuthorityMachineMutator, GrantPrincipal, GrantRecord,
};
pub use reconstruction::GrantReconstructionError;

/// Source-selected root configuration, not authentication of its supplier.
/// Never manufacture this from the grant claim in a work association.
pub struct LocalGrantConfiguration {
    pub root: PrincipalRef,
    pub namespace: EvidenceId,
    pub generation: u64,
}

impl std::fmt::Debug for LocalGrantConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalGrantConfiguration")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GrantRefusal {
    #[error("grant operation refused")]
    Denied,
    /// Revocation would remove a retained unfinished controller lineage.
    #[error("grant is required by unfinished controller work")]
    ControllerInUse,
    #[error("grant authority unavailable")]
    Unavailable,
}

pub struct LocalGrantAuthority {
    owner: Mutex<GrantAuthorityMachineAuthority>,
    publication: LocalAuthorizationPublication,
    clock: Arc<dyn LocalAuthorizationClock>,
}

impl std::fmt::Debug for LocalGrantAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalGrantAuthority")
            .finish_non_exhaustive()
    }
}

/// Freshly resolved preparation data bound to its actual owner observation.
/// It must be conjoined with the policy compiler's other owner decisions.
/// The value has no serialization or public constructor and is not an entry
/// permit. Retain it in the prepared decision; the final local boundary checks
/// its publication and the operation's independently bound deadline.
pub struct ResolvedGrant {
    restrictions: ExecutionRestrictions,
    expires_at_ms: u64,
    publication: LocalPublicationStamp,
}

impl std::fmt::Debug for ResolvedGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedGrant").finish_non_exhaustive()
    }
}

impl ResolvedGrant {
    /// Check only that the observed local owner facts are still current.
    /// This is one allocation-free atomic read. It does not check elapsed time,
    /// the operation binding or policy; those remain separate entry conditions.
    /// A changed or unavailable publication requires a fresh owner observation.
    pub fn check_current(&self) -> Result<(), PublicationError> {
        self.publication.check_current()
    }

    pub fn restrictions(&self) -> &ExecutionRestrictions {
        &self.restrictions
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
}

impl LocalGrantAuthority {
    pub(crate) fn compile_context_control(
        &self,
        binding: &meerkat_core::PreparedAuthorizationBinding,
        evaluate: impl FnOnce(
            u64,
        ) -> Result<
            Vec<crate::policy::LocalPolicyAllowance>,
            meerkat_core::OperationAuthorizationError,
        >,
    ) -> meerkat_core::authorization::ObservedAuthorizationResult<
        Arc<dyn meerkat_core::PreparedOperationAuthorization>,
    > {
        crate::work::compile_context_control(&self.publication, &self.clock, binding, evaluate)
    }

    /// Keep admission observations on this exact grant owner's publication and
    /// local clock. The caller must not recursively acquire the writer here.
    pub(crate) fn observe_controller_admission<T>(
        &self,
        evaluate: impl FnOnce(u64) -> Result<T, meerkat_core::OperationAuthorizationError>,
    ) -> Result<T, meerkat_core::OperationAuthorizationError> {
        use meerkat_core::{OperationAuthorizationError, OperationRefusalKind, OperationRefused};
        let mut known_failure = None;
        let observed = self.publication.observe(|| {
            let result = self
                .clock
                .now()
                .map_err(|_| OperationAuthorizationError::Unavailable)
                .and_then(|now| evaluate(now.unix_ms));
            if matches!(
                result,
                Err(OperationAuthorizationError::Unavailable
                    | OperationAuthorizationError::ObservationUnavailable(_))
            ) {
                known_failure = result.as_ref().err().copied();
            }
            result
        });
        if let Some(error) = known_failure {
            return Err(error);
        }
        observed
            .map_err(|error| match error {
                crate::publication::PublicationError::Changed => {
                    OperationRefused::new(OperationRefusalKind::ReprepareRequired).into()
                }
                crate::publication::PublicationError::Unavailable => {
                    OperationAuthorizationError::Unavailable
                }
            })?
            .0
    }

    pub(crate) fn reserve_controller_policy_change(
        &self,
    ) -> Result<crate::publication::LocalPublicationGuard<'_>, crate::publication::PublicationError>
    {
        self.publication.reserve_owner_change()
    }

    pub(crate) fn audited_work_context(
        &self,
        associations: Arc<
            [meerkat_authorization_contracts::work_association::InputAuthorityAssociation],
        >,
        execution_scope: meerkat_core::exact_operation::OperationExecutionScope,
        policy: Arc<dyn crate::policy::LocalWorkPolicy>,
        sink: Arc<dyn meerkat_authorization_contracts::audit::AuthorizationAuditSink>,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_core::authorization::OperationRefused,
    > {
        let inner = crate::work::LocalWorkAuthorization::new_batch(
            Arc::clone(&associations),
            policy,
            self.publication.clone(),
            Arc::clone(&self.clock),
        )?;
        let authorization = crate::audit::AuditedWorkAuthorization {
            inner: Arc::new(inner),
            associations,
            publication: self.publication.clone(),
            clock: Arc::clone(&self.clock),
            sink,
        };
        Ok(meerkat_core::authorization::WorkAuthorizationContext::new(
            Arc::new(authorization),
            execution_scope,
        ))
    }

    /// Internal composition retains this exact owner's invalidation/time
    /// sources. This is data reconstruction, never accepted native admission.
    pub(crate) fn work_context(
        &self,
        associations: Arc<
            [meerkat_authorization_contracts::work_association::InputAuthorityAssociation],
        >,
        execution_scope: meerkat_core::exact_operation::OperationExecutionScope,
        policy: Arc<dyn crate::policy::LocalWorkPolicy>,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_core::authorization::OperationRefused,
    > {
        let authorization = crate::work::LocalWorkAuthorization::new_batch(
            associations,
            policy,
            self.publication.clone(),
            Arc::clone(&self.clock),
        )?;
        Ok(meerkat_core::authorization::WorkAuthorizationContext::new(
            Arc::new(authorization),
            execution_scope,
        ))
    }

    /// Select a new process-local authority from trusted embedding configuration.
    /// Every new owner mints a fresh incarnation; configured generation reuse
    /// cannot revive a reference from an earlier owner.
    /// Restart cannot recover prior grants from associations or historical audit.
    /// A durable owner adapter must be added before claiming retained issuance.
    pub fn new(
        configuration: LocalGrantConfiguration,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
    ) -> Result<Self, GrantRefusal> {
        let incarnation = GrantAuthorityIncarnation::from_uuid(uuid::Uuid::new_v4())
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut owner = GrantAuthorityMachineAuthority::new();
        owner
            .apply(GrantAuthorityInput::Configure {
                root: principal(configuration.root)?,
                namespace: configuration.namespace,
                generation: configuration.generation,
                incarnation,
            })
            .map_err(|_| GrantRefusal::Denied)?;
        Ok(Self {
            owner: Mutex::new(owner),
            publication,
            clock,
        })
    }

    /// The caller is supplied by the authenticated administrative owner, never
    /// copied from request JSON. A represented subject is an explicit immutable
    /// root fact and remains unchanged in every child.
    pub fn issue_root(
        &self,
        caller: &PrincipalRef,
        id: EvidenceId,
        grantee: PrincipalRef,
        represented_subject: Option<PrincipalRef>,
        restrictions: ExecutionRestrictions,
    ) -> Result<GrantLineageRef, GrantRefusal> {
        let actor = principal(caller.clone())?;
        let grantee = principal(grantee)?;
        let represented_subject = represented_subject.map(principal).transpose()?;
        let mut publication = self
            .publication
            .reserve_owner_change()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        let record = GrantRecord {
            id,
            authority_incarnation: owner.state().incarnation.ok_or(GrantRefusal::Unavailable)?,
            parent: None,
            issuer: actor.clone(),
            grantee,
            represented_subject,
            issued_revision: owner
                .state()
                .revision
                .checked_add(1)
                .ok_or(GrantRefusal::Unavailable)?,
            restrictions,
        };
        owner
            .apply(GrantAuthorityInput::IssueRoot {
                actor,
                record: record.clone(),
            })
            .map_err(|_| GrantRefusal::Denied)?;
        // Generated rejection precedes every update. Successful issuance is
        // hidden by the actual owner mutex until this publication point.
        publication.publish();
        lineage_ref(&owner, &record)
    }

    /// Issue through the same owner after fresh full-chain validation. The
    /// checked mathematical value is bound to the actual retained parent.
    pub fn issue_child(
        &self,
        caller: &PrincipalRef,
        parent: &GrantLineageRef,
        id: EvidenceId,
        grantee: PrincipalRef,
        requested: ExecutionRestrictions,
    ) -> Result<GrantLineageRef, GrantRefusal> {
        let actor = principal(caller.clone())?;
        let grantee = principal(grantee)?;
        let mut publication = self
            .publication
            .reserve_owner_change()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        let parent = exact_record(&owner, parent)?.clone();
        let chain = chain(&owner, &parent)?;
        let derived = DerivedChildRestrictions::new(parent.restrictions.clone(), requested)
            .map_err(|_| GrantRefusal::Denied)?;
        let record = GrantRecord {
            id,
            authority_incarnation: owner.state().incarnation.ok_or(GrantRefusal::Unavailable)?,
            parent: Some(parent.id),
            issuer: actor.clone(),
            grantee,
            represented_subject: parent.represented_subject,
            issued_revision: owner
                .state()
                .revision
                .checked_add(1)
                .ok_or(GrantRefusal::Unavailable)?,
            restrictions: derived.effective().clone(),
        };
        let now_ms = self
            .clock
            .now()
            .map_err(|_| GrantRefusal::Unavailable)?
            .unix_ms;
        owner
            .apply(GrantAuthorityInput::IssueChild {
                actor,
                record: record.clone(),
                derived,
                chain,
                now_ms,
            })
            .map_err(|_| GrantRefusal::Denied)?;
        // Generated rejection precedes every update. Successful issuance is
        // hidden by the actual owner mutex until this publication point.
        publication.publish();
        lineage_ref(&owner, &record)
    }

    /// Revocation never deletes or reuses an issued ID. Either the configured
    /// root or the actual issuer can revoke; an expired ancestor cannot revive it.
    /// Native custody vetoes only references in unfinished controller lineages,
    /// including their ancestors. Ordinary delegation/work-use references do
    /// not prevent revocation. The veto has the distinct `ControllerInUse` result.
    /// Once custody permits mutation, busy or poisoned local publication/grant
    /// locks return `Unavailable` without waiting, mutation or publication.
    pub fn revoke(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
        custody: &mut impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody,
    ) -> Result<(), GrantRefusal> {
        use meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal;
        custody
            .with_unreferenced_controller_grant(grant, || {
                self.revoke_under_native_custody(caller, grant)
            })
            .map_err(|refusal| match refusal {
                ControllerCustodyRefusal::ControllerInUse => GrantRefusal::ControllerInUse,
                ControllerCustodyRefusal::Unavailable => GrantRefusal::Unavailable,
            })?
    }

    fn revoke_under_native_custody(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
    ) -> Result<(), GrantRefusal> {
        let actor = principal(caller.clone())?;
        let mut publication = self
            .publication
            .try_reserve_owner_change()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut owner = self
            .owner
            .try_lock()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let record = exact_record(&owner, grant)?.clone();
        let revision = owner.state().revision;
        owner
            .apply(GrantAuthorityInput::Revoke { actor, record })
            .map_err(|_| GrantRefusal::Denied)?;
        // RevokeAlready is a canonical successful no-op. Only RevokeNew
        // advances this owner's revision, so no second semantic decision is
        // made here and repeated revocation does not churn observations.
        if owner.state().revision != revision {
            publication.publish();
        }
        Ok(())
    }

    /// Resolve the exact root-to-leaf references retained by the native work
    /// owner. The executor and represented subject are separate facts. This
    /// does not authorize the requester to invoke the represented delegation.
    pub fn resolve_lineage(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
    ) -> Result<ResolvedGrant, GrantRefusal> {
        self.resolve_lineage_for_use(references, executor, represented_subject, false)
    }

    /// Resolve the actual complete controller lineage and require every
    /// ancestor's declared lifetime to be unrestricted. A finite window ending
    /// at u64::MAX is still finite and cannot satisfy this admission rule.
    pub fn resolve_controller_lineage(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
    ) -> Result<ResolvedGrant, GrantRefusal> {
        self.resolve_lineage_for_use(references, executor, represented_subject, true)
    }

    fn resolve_lineage_for_use(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
        controller: bool,
    ) -> Result<ResolvedGrant, GrantRefusal> {
        let (resolved, publication) = self
            .publication
            .observe(|| {
                self.resolve_lineage_data(references, executor, represented_subject, controller)
            })
            .map_err(|_| GrantRefusal::Unavailable)?;
        let (restrictions, expires_at_ms) = resolved?;
        Ok(ResolvedGrant {
            restrictions,
            expires_at_ms,
            publication,
        })
    }

    // Called only inside the actual publication observation above. Resolution
    // prepares a disposable view; it does not run on the warm entry-check path.
    fn resolve_lineage_data(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
        controller: bool,
    ) -> Result<(ExecutionRestrictions, u64), GrantRefusal> {
        let executor = principal(executor.clone())?;
        let represented_subject = represented_subject.cloned().map(principal).transpose()?;
        let now_ms = self
            .clock
            .now()
            .map_err(|_| GrantRefusal::Unavailable)?
            .unix_ms;
        let mut owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        let reference = references.last().ok_or(GrantRefusal::Denied)?;
        let leaf = exact_record(&owner, reference)?.clone();
        let chain = chain(&owner, &leaf)?;
        if chain.len() != references.len() {
            return Err(GrantRefusal::Denied);
        }
        for (record, reference) in chain.iter().zip(references) {
            if lineage_ref(&owner, record)? != *reference {
                return Err(GrantRefusal::Denied);
            }
        }
        if controller
            && chain.iter().any(|record| {
                !matches!(
                    record.restrictions.lifetime.bound(),
                    LifetimeBound::Unrestricted
                )
            })
        {
            return Err(GrantRefusal::Denied);
        }
        // Pure projection of accepted retained bounds, not a second permission
        // decision. Compute before moving the chain into its generated owner.
        let expires_at_ms = chain
            .iter()
            .filter_map(|record| match record.restrictions.lifetime.bound() {
                LifetimeBound::Window { expires_at_ms, .. } => Some(expires_at_ms),
                _ => None,
            })
            .min()
            .unwrap_or(u64::MAX);
        let transition = owner
            .apply(GrantAuthorityInput::ResolveUse {
                namespace: reference.authority_namespace.clone(),
                generation: reference.authority_generation,
                incarnation: reference.authority_incarnation,
                executor,
                represented_subject,
                leaf,
                chain,
                now_ms,
            })
            .map_err(|_| GrantRefusal::Denied)?;
        let restrictions = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                GrantAuthorityEffect::UseResolved { leaf } => Some(leaf.restrictions.clone()),
                _ => None,
            })
            .ok_or(GrantRefusal::Unavailable)?;
        Ok((restrictions, expires_at_ms))
    }
}

fn principal(principal: PrincipalRef) -> Result<GrantPrincipal, GrantRefusal> {
    GrantPrincipal::new(principal).map_err(|_| GrantRefusal::Denied)
}

fn lineage_ref(
    owner: &GrantAuthorityMachineAuthority,
    record: &GrantRecord,
) -> Result<GrantLineageRef, GrantRefusal> {
    Ok(GrantLineageRef {
        root_authority: owner
            .state()
            .root
            .as_ref()
            .ok_or(GrantRefusal::Unavailable)?
            .principal()
            .clone(),
        authority_namespace: owner
            .state()
            .namespace
            .clone()
            .ok_or(GrantRefusal::Unavailable)?,
        authority_generation: owner.state().generation,
        authority_incarnation: owner.state().incarnation.ok_or(GrantRefusal::Unavailable)?,
        grant_id: record.id.clone(),
        issued_revision: record.issued_revision,
    })
}

fn exact_record<'a>(
    owner: &'a GrantAuthorityMachineAuthority,
    reference: &GrantLineageRef,
) -> Result<&'a GrantRecord, GrantRefusal> {
    let record = owner
        .state()
        .records
        .get(&reference.grant_id)
        .ok_or(GrantRefusal::Denied)?;
    if lineage_ref(owner, record)? != *reference {
        return Err(GrantRefusal::Denied);
    }
    Ok(record)
}

// Traversal only extracts retained rows. The generated transition checks every
// row, parent/issuer/rank relation, revocation and current lifetime itself.
fn chain(
    owner: &GrantAuthorityMachineAuthority,
    leaf: &GrantRecord,
) -> Result<Vec<GrantRecord>, GrantRefusal> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = leaf;
    loop {
        if result.len() >= 64 || !seen.insert(current.id.clone()) {
            return Err(GrantRefusal::Unavailable);
        }
        result.push(current.clone());
        let Some(parent) = &current.parent else {
            break;
        };
        current = owner
            .state()
            .records
            .get(parent)
            .ok_or(GrantRefusal::Unavailable)?;
    }
    result.reverse();
    Ok(result)
}

#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod tests;

/// Grant/compiler unit fixtures have no actual native runtime attached. Never
/// compile this adapter into a host or use it as native admission evidence.
#[cfg(test)]
pub(crate) struct IsolatedGrantTestCustody;

#[cfg(test)]
impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for IsolatedGrantTestCustody
{
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _reference: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<
        Result<T, E>,
        meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
    > {
        Ok(mutate())
    }
}

#[cfg(test)]
mod reconstruction_tests;
