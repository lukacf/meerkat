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
use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_core::auth::PrincipalRef;

use crate::clock::LocalAuthorizationClock;
use crate::publication::LocalAuthorizationPublication;

pub(crate) mod dsl;
use dsl::{
    GrantAuthorityEffect, GrantAuthorityInput, GrantAuthorityMachineAuthority,
    GrantAuthorityMachineMutator, GrantPrincipal, GrantRecord,
};

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

/// Freshly resolved data, not an entry permit. It must remain under the calling
/// policy compiler's publication observation and be conjoined with its other
/// owner decisions. The value has no serialization or public constructor.
pub struct ResolvedGrant {
    restrictions: ExecutionRestrictions,
    expires_at_ms: u64,
}

impl std::fmt::Debug for ResolvedGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedGrant").finish_non_exhaustive()
    }
}

impl ResolvedGrant {
    pub fn restrictions(&self) -> &ExecutionRestrictions {
        &self.restrictions
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
}

impl LocalGrantAuthority {
    /// Select a new process-local authority from trusted embedding configuration.
    /// Restart cannot recover prior grants from associations or historical audit.
    /// A durable owner adapter must be added before claiming retained issuance.
    pub fn new(
        configuration: LocalGrantConfiguration,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
    ) -> Result<Self, GrantRefusal> {
        let mut owner = GrantAuthorityMachineAuthority::new();
        owner
            .apply(GrantAuthorityInput::Configure {
                root: principal(configuration.root)?,
                namespace: configuration.namespace,
                generation: configuration.generation,
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
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        let record = GrantRecord {
            id,
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
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        let parent = exact_record(&owner, parent)?.clone();
        let chain = chain(&owner, &parent)?;
        let derived = DerivedChildRestrictions::new(parent.restrictions.clone(), requested)
            .map_err(|_| GrantRefusal::Denied)?;
        let record = GrantRecord {
            id,
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
        lineage_ref(&owner, &record)
    }

    /// Revocation never deletes or reuses an issued ID. Either the configured
    /// root or the actual issuer can revoke; an expired ancestor cannot revive it.
    pub fn revoke(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
        custody: &mut impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody,
    ) -> Result<(), GrantRefusal> {
        use meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal;
        custody
            .with_unreferenced_grant(grant, || self.revoke_under_native_custody(caller, grant))
            .map_err(|refusal| match refusal {
                ControllerCustodyRefusal::ReferencedByUnfinishedWork => GrantRefusal::Denied,
                ControllerCustodyRefusal::Unavailable => GrantRefusal::Unavailable,
            })?
    }

    fn revoke_under_native_custody(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
    ) -> Result<(), GrantRefusal> {
        let actor = principal(caller.clone())?;
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        let record = exact_record(&owner, grant)?.clone();
        owner
            .apply(GrantAuthorityInput::Revoke { actor, record })
            .map_err(|_| GrantRefusal::Denied)?;
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
        let executor = principal(executor.clone())?;
        let represented_subject = represented_subject.cloned().map(principal).transpose()?;
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
        let now_ms = self
            .clock
            .now()
            .map_err(|_| GrantRefusal::Unavailable)?
            .unix_ms;
        let transition = owner
            .apply(GrantAuthorityInput::ResolveUse {
                namespace: reference.authority_namespace.clone(),
                generation: reference.authority_generation,
                executor,
                represented_subject,
                leaf,
                chain: chain.clone(),
                now_ms,
            })
            .map_err(|_| GrantRefusal::Denied)?;
        let resolved = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                GrantAuthorityEffect::UseResolved { leaf } => Some(leaf.clone()),
                _ => None,
            })
            .ok_or(GrantRefusal::Unavailable)?;
        // Pure projection of accepted retained bounds, not a second permission
        // decision. Operation matching remains the existing restriction algebra.
        let expires_at_ms = chain
            .iter()
            .filter_map(|record| match record.restrictions.lifetime.bound() {
                LifetimeBound::Window { expires_at_ms, .. } => Some(expires_at_ms),
                _ => None,
            })
            .min()
            .unwrap_or(u64::MAX);
        Ok(ResolvedGrant {
            restrictions: resolved.restrictions,
            expires_at_ms,
        })
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
mod tests;

/// Grant/compiler unit fixtures have no actual native runtime attached. Never
/// compile this adapter into a host or use it as native admission evidence.
#[cfg(test)]
pub(crate) struct IsolatedGrantTestCustody;

#[cfg(test)]
impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for IsolatedGrantTestCustody
{
    fn with_unreferenced_grant<T, E>(
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
