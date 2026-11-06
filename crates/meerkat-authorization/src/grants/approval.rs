//! Approval-carried children: grants issued by executing an approved durable
//! decision, and the spend of their uses.
//!
//! A grant may carry a child rule ([`ChildApprovalRule`]): every child of it
//! then carries the decision a person made for the rule's approver id
//! ([`GrantApproval`]). Such a child serves one requester and one work, the
//! exact operation when one is bound, and a counted number of uses; it
//! cannot be delegated further and ends in a finite window. The generated
//! owner decides all of it; this module builds the owner's inputs and keeps
//! no second record of any of it.
//!
//! A use is resolved like any lineage, with the use's requester, work and
//! operation ([`GrantUse`]). It is spent before its operation enters
//! ([`LocalGrantAuthority::consume_use`]): with durable custody the spend is
//! acknowledged by the store first, and advances the owner's revision, so a
//! decision prepared before the spend is stale after it.

use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DomainRestriction, ExactRestriction, ExecutionRestrictions,
    LifetimeBound, ProcessorRef,
};
use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_authorization_contracts::work_association::OriginalWorkRef;
use meerkat_core::approval::decision::{ApproverId, Sha256Digest};
use meerkat_core::auth::PrincipalRef;
use serde::Serialize;

use super::dsl::{
    ChildApprovalRule, GrantApproval, GrantAuthorityInput, GrantDecisionAssurance,
    GrantDecisionTier, GrantDeviceTrust, GrantOperationBinding, GrantPrincipal, GrantRecord,
};
use super::{
    GrantRefusal, LineageResolutionRefusal, LineageUse, LocalGrantAuthority, ResolvedGrant,
    exact_record, principal,
};

/// One use of a lineage: the requester the work serves, the work, and the
/// exact operation, as the native composition states them. A leaf that
/// carries an approval resolves only for its own requester and work, and
/// for its bound operation when it binds one.
#[derive(Debug, Clone, Copy)]
pub struct GrantUse<'a> {
    pub requester: &'a PrincipalRef,
    pub work: &'a OriginalWorkRef,
    /// The operation's exact binding, when its owner supplies one. A use
    /// without one never matches a leaf that binds an operation.
    pub operation: Option<&'a GrantOperationBinding>,
}

/// A use's facts as the generated owner's inputs carry them.
pub(super) struct UseFacts {
    pub(super) requester: Option<GrantPrincipal>,
    pub(super) work: Option<OriginalWorkRef>,
    pub(super) operation: Option<GrantOperationBinding>,
}

impl UseFacts {
    /// No use facts: a plain use, which never resolves an approval leaf.
    pub(super) fn none() -> Self {
        Self {
            requester: None,
            work: None,
            operation: None,
        }
    }

    pub(super) fn of(use_: GrantUse<'_>) -> Result<Self, GrantRefusal> {
        Ok(Self {
            requester: Some(principal(use_.requester.clone())?),
            work: Some(use_.work.clone()),
            operation: use_.operation.cloned(),
        })
    }
}

/// The terms an approval-carried child is issued with: what the access offer
/// asked for and what the issued record states, in one canonical form. The
/// window is its length, since the offer is held before the approval that
/// starts it. Its digest is the approved request's digest exactly when the
/// issued terms equal the approved ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantChildTerms {
    /// The parent grant.
    pub parent: EvidenceId,
    /// The parent's grantee, issuing the child.
    pub issuer: PrincipalRef,
    pub grantee: PrincipalRef,
    /// The parent's represented subject, kept by every child.
    pub represented_subject: Option<PrincipalRef>,
    pub actions: ExactRestriction<ActionRef>,
    pub resource_domains: DomainRestriction,
    pub processors: ExactRestriction<ProcessorRef>,
    pub audiences: ExactRestriction<AudienceRef>,
    /// The length of the child's window, in milliseconds.
    pub window_ms: u64,
    pub operation: Option<GrantOperationBinding>,
    pub uses: u64,
    pub requester: PrincipalRef,
    pub work: OriginalWorkRef,
    pub review_before_return: Option<ApproverId>,
}

#[derive(Serialize)]
struct CanonicalTerms<'a> {
    format: &'static str,
    terms: &'a GrantChildTerms,
}

impl GrantChildTerms {
    /// The terms an issued approval-carried record states: its parent,
    /// issuer, grantee and subject, its restrictions but for the window,
    /// which is its length, and its approval's use terms. `None` for a
    /// record that carries no approval or has no finite window.
    #[must_use]
    pub fn of_record(record: &GrantRecord) -> Option<Self> {
        let approval = record.approval.as_ref()?;
        let LifetimeBound::Window {
            not_before_ms,
            expires_at_ms,
        } = record.restrictions.lifetime.bound()
        else {
            return None;
        };
        Some(Self {
            parent: record.parent.clone()?,
            issuer: record.issuer.principal().clone(),
            grantee: record.grantee.principal().clone(),
            represented_subject: record
                .represented_subject
                .as_ref()
                .map(|subject| subject.principal().clone()),
            actions: record.restrictions.actions.clone(),
            resource_domains: record.restrictions.resource_domains.clone(),
            processors: record.restrictions.processors.clone(),
            audiences: record.restrictions.audiences.clone(),
            window_ms: expires_at_ms.checked_sub(not_before_ms)?,
            operation: approval.operation.clone(),
            uses: approval.uses,
            requester: approval.requester.principal().clone(),
            work: approval.work.clone(),
            review_before_return: approval.review_before_return.clone(),
        })
    }
}

/// The digest of `terms`: SHA-256 of their canonical JSON, under the format
/// `grant.child.terms.v1`. The one function both the access offer (for its
/// held request) and the grant owner (for the record it issues) use, so the
/// two are equal exactly when the issued terms are the approved ones.
/// `None` only if the terms cannot be encoded.
#[must_use]
pub fn grant_child_terms_digest(terms: &GrantChildTerms) -> Option<Sha256Digest> {
    let canonical = CanonicalTerms {
        format: "grant.child.terms.v1",
        terms,
    };
    serde_json::to_vec(&canonical)
        .ok()
        .map(|bytes| Sha256Digest::of(&bytes))
}

/// The terms digest an issuance gives the generated owner: the approval's
/// terms digest for an approval-carried record, else the digest of the
/// record itself (the owner reads it only for an approval).
pub(super) fn issued_terms_digest(record: &GrantRecord) -> Result<Sha256Digest, GrantRefusal> {
    if record.approval.is_some() {
        return GrantChildTerms::of_record(record)
            .as_ref()
            .and_then(grant_child_terms_digest)
            .ok_or(GrantRefusal::Denied);
    }
    serde_json::to_vec(record)
        .map(|bytes| Sha256Digest::of(&bytes))
        .map_err(|_| GrantRefusal::Unavailable)
}

/// Why an approval-carried child was not issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalChildRefusal {
    /// The parent is not a current grant the caller may issue from now
    /// (revoked, expired, its depth spent, or another grantee), or its
    /// bounds refuse the requested ones.
    #[error("parent grant refused the child")]
    ParentRefused,
    /// The parent carries no child rule, and only a grant under a rule has
    /// approval-carried children.
    #[error("the parent carries no child approval rule")]
    NoChildRule,
    /// The decision does not meet the parent's rule: another approver id,
    /// not a person's decision, or an assurance the rule does not accept.
    #[error("the decision does not meet the parent's child rule")]
    BelowRule,
    /// The child's terms are not the approved request's.
    #[error("the child's terms differ from the approved request")]
    TermsDiffer,
    /// The child could be delegated, has no finite window, or serves no use.
    #[error("the child's terms are unbounded")]
    Unbounded,
    /// Another grant already carries the decision.
    #[error("the decision already carries a grant")]
    DecisionCarried,
    /// The id names a grant this owner already issued.
    #[error("grant id already issued")]
    AlreadyIssued,
    /// The owner cannot say now; nothing was issued.
    #[error("grant authority unavailable")]
    Unavailable,
}

/// Why a use was not spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConsumeUseRefusal {
    /// Not a use this grant serves now: not current, another requester,
    /// work or operation, spent, or no approval. Nothing was spent.
    #[error("grant use refused")]
    Denied,
    /// The approver of the grant's decision is no longer current. Nothing
    /// was spent.
    #[error("the grant's approver is no longer current")]
    ApproverNotCurrent,
    /// The owner cannot say now (contention, clock, custody). Nothing was
    /// spent.
    #[error("grant authority unavailable")]
    Unavailable,
    /// The store's acknowledgement of the spend is unknown: the use counts
    /// as spent, the operation must not enter, and the owner is unavailable
    /// until it is restored.
    #[error("grant use spend outcome unknown")]
    Uncertain,
}

impl LocalGrantAuthority {
    /// [`issue_root`](Self::issue_root) of a root whose children must each
    /// carry a person's decision for `rule`'s approver id.
    pub fn issue_root_with_child_rule(
        &self,
        caller: &PrincipalRef,
        id: EvidenceId,
        grantee: PrincipalRef,
        represented_subject: Option<PrincipalRef>,
        restrictions: ExecutionRestrictions,
        rule: ChildApprovalRule,
    ) -> Result<GrantLineageRef, GrantRefusal> {
        self.issue_root_carrying(
            caller,
            id,
            grantee,
            represented_subject,
            restrictions,
            Some(rule),
        )
    }

    /// [`issue_child`](Self::issue_child) of a child whose own children must
    /// each carry a person's decision for `rule`'s approver id. Its parent's
    /// rule, if any, still applies to it.
    pub fn issue_child_with_child_rule(
        &self,
        caller: &PrincipalRef,
        parent: &GrantLineageRef,
        id: EvidenceId,
        grantee: PrincipalRef,
        requested: ExecutionRestrictions,
        rule: ChildApprovalRule,
    ) -> Result<GrantLineageRef, GrantRefusal> {
        self.issue_child_carrying(caller, parent, id, grantee, requested, Some(rule), None)
    }

    /// Issue the approval-carried child of `parent` that executing an
    /// approved decision produces: `approval` comes from the decision's
    /// execution permit. The generated owner refuses a parent without a
    /// child rule, a decision below the rule, terms other than the approved
    /// request's, a child that could be delegated or has no window or no
    /// use, and a decision another grant carries.
    ///
    /// # Errors
    /// The typed [`ApprovalChildRefusal`]; nothing is issued.
    pub fn issue_approval_child(
        &self,
        caller: &PrincipalRef,
        parent: &GrantLineageRef,
        id: EvidenceId,
        grantee: PrincipalRef,
        requested: ExecutionRestrictions,
        approval: GrantApproval,
    ) -> Result<GrantLineageRef, ApprovalChildRefusal> {
        self.classify_approval_child(caller, parent, &id, &grantee, &requested, &approval)?;
        self.issue_child_carrying(caller, parent, id, grantee, requested, None, Some(approval))
            .map_err(|refusal| match refusal {
                GrantRefusal::Duplicate => ApprovalChildRefusal::AlreadyIssued,
                GrantRefusal::Unavailable => ApprovalChildRefusal::Unavailable,
                GrantRefusal::Denied | GrantRefusal::ControllerInUse => {
                    ApprovalChildRefusal::ParentRefused
                }
            })
    }

    /// The typed reason the generated owner would refuse this approval
    /// child, read from the retained parent before the issuance. It decides
    /// nothing: the owner decides at the issuance, and a refusal this read
    /// does not foresee is the parent's.
    fn classify_approval_child(
        &self,
        caller: &PrincipalRef,
        parent: &GrantLineageRef,
        id: &EvidenceId,
        grantee: &PrincipalRef,
        requested: &ExecutionRestrictions,
        approval: &GrantApproval,
    ) -> Result<(), ApprovalChildRefusal> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| ApprovalChildRefusal::Unavailable)?;
        let parent_record = exact_record(&owner, parent)
            .map_err(|refusal| match refusal {
                GrantRefusal::Unavailable => ApprovalChildRefusal::Unavailable,
                _ => ApprovalChildRefusal::ParentRefused,
            })?
            .clone();
        if owner.state().records.contains_key(id) {
            return Err(ApprovalChildRefusal::AlreadyIssued);
        }
        if owner
            .state()
            .carried_decisions
            .contains(&approval.decision_id)
        {
            return Err(ApprovalChildRefusal::DecisionCarried);
        }
        drop(owner);
        let Some(rule) = parent_record.child_approval.as_ref() else {
            return Err(ApprovalChildRefusal::NoChildRule);
        };
        let assurance_meets_rule = match approval.assurance {
            GrantDecisionAssurance::PersonAuthenticated => true,
            GrantDecisionAssurance::DeviceTrusted => {
                rule.accepts_device_trust == GrantDeviceTrust::Accepted
            }
            GrantDecisionAssurance::ServiceCredential | GrantDecisionAssurance::Unattested => false,
        };
        if approval.approver_id != rule.approver_id
            || approval.tier != GrantDecisionTier::Human
            || !assurance_meets_rule
        {
            return Err(ApprovalChildRefusal::BelowRule);
        }
        let derived =
            DerivedChildRestrictions::new(parent_record.restrictions.clone(), requested.clone())
                .map_err(|_| ApprovalChildRefusal::ParentRefused)?;
        let effective = derived.effective();
        let bounded = matches!(
            effective.delegation_depth.bound(),
            meerkat_authorization_contracts::constraints::DepthBound::Remaining(0)
        ) && effective.delegation_depth.unresolved_facts().is_empty()
            && effective.lifetime.unresolved_facts().is_empty()
            && matches!(effective.lifetime.bound(), LifetimeBound::Window { .. })
            && approval.uses >= 1;
        if !bounded {
            return Err(ApprovalChildRefusal::Unbounded);
        }
        let issued = GrantRecord {
            id: id.clone(),
            authority_incarnation: parent_record.authority_incarnation,
            parent: Some(parent_record.id.clone()),
            issuer: principal(caller.clone()).map_err(|_| ApprovalChildRefusal::ParentRefused)?,
            grantee: principal(grantee.clone()).map_err(|_| ApprovalChildRefusal::ParentRefused)?,
            represented_subject: parent_record.represented_subject.clone(),
            issued_revision: 0,
            restrictions: effective.clone(),
            child_approval: None,
            approval: Some(approval.clone()),
        };
        let issued_digest = GrantChildTerms::of_record(&issued)
            .as_ref()
            .and_then(grant_child_terms_digest);
        if issued_digest.as_ref() != Some(&approval.request_digest) {
            return Err(ApprovalChildRefusal::TermsDiffer);
        }
        Ok(())
    }

    /// [`resolve_lineage`](Self::resolve_lineage) for one use: a leaf that
    /// carries an approval resolves only for the use's requester and work,
    /// its bound operation, and while it has an unspent use.
    pub fn resolve_lineage_with_use(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
        use_: GrantUse<'_>,
    ) -> Result<ResolvedGrant, GrantRefusal> {
        self.resolve_lineage_with_use_observed(references, executor, represented_subject, use_)
            .map_err(super::standalone_refusal)
    }

    /// [`resolve_lineage_with_use`](Self::resolve_lineage_with_use) in the
    /// observed form; see
    /// [`resolve_lineage_observed`](Self::resolve_lineage_observed).
    ///
    /// # Errors
    /// As [`resolve_lineage_observed`](Self::resolve_lineage_observed).
    pub fn resolve_lineage_with_use_observed(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
        use_: GrantUse<'_>,
    ) -> Result<ResolvedGrant, LineageResolutionRefusal> {
        self.resolve_lineage_observed_for_use(
            references,
            LineageUse::Executor {
                executor,
                represented_subject,
                use_facts: Some(use_),
            },
            false,
        )
    }

    /// Spend one use of the approval-carried leaf of `references`, before
    /// the use's operation enters: the checks of
    /// [`resolve_lineage_with_use`](Self::resolve_lineage_with_use) at a fresh
    /// clock reading, and `approver_current`, the host's answer that the
    /// person who decided the leaf's decision still holds its approver id.
    /// With durable custody the spend is acknowledged by the store before
    /// this returns `Ok`; it advances the owner's revision and publishes, so
    /// every decision prepared before it is stale. Never waits: contention
    /// refuses as unavailable.
    ///
    /// # Errors
    /// The typed [`ConsumeUseRefusal`]. Only `Uncertain` may have spent the
    /// use; the operation must not enter on any refusal.
    pub fn consume_use(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
        use_: GrantUse<'_>,
        approver_current: bool,
    ) -> Result<(), ConsumeUseRefusal> {
        let denied = |_: GrantRefusal| ConsumeUseRefusal::Denied;
        let facts = UseFacts::of(use_).map_err(denied)?;
        let executor = principal(executor.clone()).map_err(denied)?;
        let represented_subject = represented_subject
            .cloned()
            .map(principal)
            .transpose()
            .map_err(denied)?;
        let mut publication = self
            .publication
            .try_reserve_owner_change()
            .map_err(|_| ConsumeUseRefusal::Unavailable)?;
        let mut owner = self
            .owner
            .try_lock()
            .map_err(|_| ConsumeUseRefusal::Unavailable)?;
        if let Some(durable) = &self.durable {
            durable
                .check_available()
                .map_err(|_| ConsumeUseRefusal::Unavailable)?;
        }
        let reference = references.last().ok_or(ConsumeUseRefusal::Denied)?;
        let leaf = exact_record(&owner, reference).map_err(denied)?.clone();
        let chain = super::chain(&owner, &leaf).map_err(|_| ConsumeUseRefusal::Unavailable)?;
        if chain.len() != references.len() {
            return Err(ConsumeUseRefusal::Denied);
        }
        for (record, reference) in chain.iter().zip(references) {
            if super::lineage_ref(&owner, record).map_err(denied)? != *reference {
                return Err(ConsumeUseRefusal::Denied);
            }
        }
        let now_ms = self
            .clock
            .now()
            .map_err(|_| ConsumeUseRefusal::Unavailable)?
            .unix_ms;
        let revision = owner.state().revision;
        let applied = self.apply_change(
            &mut owner,
            &mut publication,
            GrantAuthorityInput::ConsumeUse {
                namespace: reference.authority_namespace.clone(),
                generation: reference.authority_generation,
                incarnation: reference.authority_incarnation,
                executor,
                represented_subject,
                requester: facts.requester,
                work: facts.work,
                operation: facts.operation,
                leaf,
                chain,
                now_ms,
                approver_current,
            },
        );
        if let Err(refusal) = applied {
            let uncertain = self
                .durable
                .as_ref()
                .is_some_and(|durable| durable.check_available().is_err());
            return Err(if uncertain {
                ConsumeUseRefusal::Uncertain
            } else if refusal == GrantRefusal::Unavailable {
                ConsumeUseRefusal::Unavailable
            } else {
                ConsumeUseRefusal::Denied
            });
        }
        if owner.state().revision == revision {
            // The generated owner's refusal arm for an approver who is no
            // longer current: nothing spent, nothing published.
            return Err(ConsumeUseRefusal::ApproverNotCurrent);
        }
        // Spent and acknowledged: every observation prepared before the
        // spend fails its final currentness check.
        let _ = publication.publish();
        Ok(())
    }
}
