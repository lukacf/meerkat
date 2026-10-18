//! Canonical process-local grant issuance, revocation and full-lineage use.
//!
//! This host retains the generated owner directly. It is not a second grant
//! registry. Process-restart custody is the optional `durable` seam owned by
//! this same owner. Administrative root selection and authenticated callers
//! come from the embedding's actual owners.
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
use crate::publication::{
    LocalAuthorizationPublication, LocalPublicationGuard, LocalPublicationStamp, PublicationError,
};

pub(crate) mod dsl;
mod durable;
mod reconstruction;
use dsl::{
    GrantAuthorityEffect, GrantAuthorityInput, GrantAuthorityMachineAuthority,
    GrantAuthorityMachineMutator, GrantPrincipal, GrantRecord,
};
#[cfg(test)]
pub(crate) use durable::with_last_issued_restrictions;
pub use durable::{
    DurableGrantOwner, FileGrantOwnerStore, GrantDurabilityError, GrantOwnerChange,
    GrantOwnerIdentity, GrantOwnerStore, GrantStoreDurability, GrantStoreError,
    InMemoryGrantOwnerStore,
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
    /// An issuance the caller may perform names an id this owner already
    /// issued. Issued ids are never reused, revoked ones included.
    #[error("grant id already issued")]
    Duplicate,
    /// Revocation would remove a retained unfinished controller lineage.
    #[error("grant is required by unfinished controller work")]
    ControllerInUse,
    #[error("grant authority unavailable")]
    Unavailable,
}

/// Why a lineage resolution read inside the caller's own enclosing
/// observation of this owner's publication has no grant. An owner change
/// published during the resolution stays apart from the owner's refusals:
/// the enclosing observation is stale too, so the caller classifies it as
/// a stale read (for an operation, `OperationRefusalKind::ReprepareRequired`)
/// and may rebuild once, never as unavailability. A standalone resolution
/// ([`LocalGrantAuthority::resolve_lineage`] and its siblings) reports such
/// a change as [`GrantRefusal::Unavailable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LineageResolutionRefusal {
    /// An owner change published during this resolution's observation.
    #[error("grant owner facts changed during the resolution")]
    Changed,
    /// The owner's own refusal, as the standalone resolution reports it.
    #[error(transparent)]
    Refused(GrantRefusal),
}

/// A standalone caller has no enclosing observation to rebuild: a
/// concurrent change is "this owner cannot say now".
fn standalone_refusal(refusal: LineageResolutionRefusal) -> GrantRefusal {
    match refusal {
        LineageResolutionRefusal::Changed => GrantRefusal::Unavailable,
        LineageResolutionRefusal::Refused(refusal) => refusal,
    }
}

/// One intended child grant, retained by its caller with a stable id before
/// any attempt: the exact root-to-parent lineage of the independent parent
/// grant, the caller issuing from it (that parent's grantee), the child's
/// grantee and represented subject (the parent lineage's own), and the
/// requested bounds. A retry or recovery presents the same intent; a
/// replacement id is never allocated to escape an uncertain append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildGrantIntent {
    pub issuer: PrincipalRef,
    pub parent_lineage: Vec<GrantLineageRef>,
    pub id: EvidenceId,
    pub grantee: PrincipalRef,
    pub represented_subject: Option<PrincipalRef>,
    pub requested: ExecutionRestrictions,
}

/// The child of a [`ChildGrantIntent`], usable now: its complete
/// root-to-child lineage. Use still requires the caller's own current checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildGrantOutcome {
    /// Issued and acknowledged by this call.
    Issued { lineage: Vec<GrantLineageRef> },
    /// This owner already retained exactly the intended child (an earlier
    /// attempt landed); nothing was appended.
    Recovered { lineage: Vec<GrantLineageRef> },
}

impl ChildGrantOutcome {
    #[must_use]
    pub fn lineage(&self) -> &[GrantLineageRef] {
        match self {
            Self::Issued { lineage } | Self::Recovered { lineage } => lineage,
        }
    }
}

/// The window in which a caller's conjunct holds: from `not_before_ms` to
/// before `expires_at_ms` on the wall clock, and before `deadline` on the
/// monotonic clock of the same native time read. The final checks of an
/// append or a recovery require all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildGrantValidity {
    pub not_before_ms: u64,
    pub expires_at_ms: u64,
    pub deadline: meerkat_core::time_compat::Instant,
}

/// Why a [`ChildGrantIntent`] has no usable child now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChildGrantRefusal {
    /// Not permitted now: the parent lineage, the caller's further conjunct
    /// or the requested bounds refuse it, or the id names another grant. No
    /// child was appended by this call.
    #[error("child grant refused")]
    Denied,
    /// The owners cannot say now (contention, clock, custody). No child was
    /// appended by this call.
    #[error("child grant authority unavailable")]
    Unavailable,
    /// The append started and its outcome is unknown. The owner is
    /// unavailable until restored; recover with the same intent then.
    #[error("child grant append outcome unknown")]
    Uncertain,
}

/// The refusal of [`LocalGrantAuthority::revoke_expecting`]. Only
/// [`NotCurrent`](Self::NotCurrent) guarantees that nothing changed and
/// nothing was published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExpectedRevokeRefusal {
    /// The expected stamp is not a current stamp of this owner's
    /// publication: it was observed on another publication, or an owner
    /// change has published since. Decided under the revocation's own
    /// publication reservation, before any mutation: nothing changed and
    /// nothing was published. Observe the owners again before any retry.
    #[error("expected publication stamp is not current for this grant owner")]
    NotCurrent,
    /// [`LocalGrantAuthority::revoke`]'s own outcome, with its meaning. It
    /// does not imply that the revocation had no effect:
    /// `Grant(GrantRefusal::Unavailable)` can follow an uncertain durable
    /// append, which may have made the revocation durable.
    #[error(transparent)]
    Grant(#[from] GrantRefusal),
}

pub struct LocalGrantAuthority {
    owner: Mutex<GrantAuthorityMachineAuthority>,
    publication: LocalAuthorizationPublication,
    clock: Arc<dyn LocalAuthorizationClock>,
    durable: Option<durable::DurableCustody>,
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
    /// Each link's retained bounds, root to leaf, for conjuncts that must
    /// hold across the lineage rather than only in the leaf's conjunction
    /// (an MCP connector approval).
    lineage: Vec<ExecutionRestrictions>,
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
    /// Each link's retained bounds, root to leaf.
    pub fn lineage_restrictions(&self) -> &[ExecutionRestrictions] {
        &self.lineage
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
}

/// The ceiling a contributor's lineage imposes, resolved for its own leaf
/// ([`LocalGrantAuthority::resolve_lineage_ceiling`]): its restrictions, its
/// earliest expiry and the currency of the owner observation that resolved
/// it. It is not a grant and carries no lineage or approval evidence: it has
/// no public constructor, no serialization and no conversion to a
/// [`ResolvedGrant`], so it cannot be presented where a grant is required.
///
/// ```compile_fail,E0308
/// use meerkat_authorization::grants::ContributorCeiling;
/// use meerkat_authorization::mcp_installation::{
///     McpInstallationRef, check_resolved_installation,
/// };
///
/// fn as_installation_evidence(ceiling: &ContributorCeiling, installation: &McpInstallationRef) {
///     let _ = check_resolved_installation(ceiling, installation, 0);
/// }
/// ```
pub struct ContributorCeiling {
    restrictions: ExecutionRestrictions,
    expires_at_ms: u64,
    publication: LocalPublicationStamp,
}

impl std::fmt::Debug for ContributorCeiling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContributorCeiling").finish_non_exhaustive()
    }
}

impl ContributorCeiling {
    pub fn restrictions(&self) -> &ExecutionRestrictions {
        &self.restrictions
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
    /// The owner facts this ceiling was resolved from are still current: the
    /// same check as [`ResolvedGrant::check_current`].
    pub fn check_current(&self) -> Result<(), PublicationError> {
        self.publication.check_current()
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
    /// This owner is process-local; use `new_durable` and `restore` for retained
    /// issuance across restart.
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
            durable: None,
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
        // One MCP connector approval per server, whichever form names it.
        if !crate::mcp_installation::lineage_approvals_agree(std::iter::once(&restrictions)) {
            return Err(GrantRefusal::Denied);
        }
        let mut publication = self
            .publication
            .reserve_owner_change()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        let may_issue = owner.state().root.as_ref() == Some(&actor);
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
        self.apply_change(
            &mut owner,
            &mut publication,
            GrantAuthorityInput::IssueRoot {
                actor,
                record: record.clone(),
            },
        )
        .map_err(|refusal| issue_refusal(&owner, refusal, &record.id, may_issue))?;
        // Generated rejection precedes every update. Successful issuance is
        // hidden by the actual owner mutex until this publication point. A
        // poison since the reservation leaves it retained but unusable until
        // reconstruction.
        let _ = publication.publish();
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
        let may_issue = parent.grantee == actor;
        let chain = chain(&owner, &parent)?;
        let derived = DerivedChildRestrictions::new(parent.restrictions.clone(), requested)
            .map_err(|_| GrantRefusal::Denied)?;
        // A child names no MCP connector approval its lineage does not: an
        // approval recorded in one form cannot be replaced through the other.
        let lineage = chain
            .iter()
            .map(|record| &record.restrictions)
            .chain(std::iter::once(derived.effective()));
        if !crate::mcp_installation::lineage_approvals_agree(lineage) {
            return Err(GrantRefusal::Denied);
        }
        // Only the operator-issued root records an approval, and a child
        // keeps or narrows one its actual parent still holds: it never
        // originates or switches one, nor regains one a parent removed.
        let root = chain
            .first()
            .map_or(&parent.restrictions, |root| &root.restrictions);
        if !crate::mcp_installation::child_approvals_from(root, derived.effective())
            || !crate::mcp_installation::child_approvals_from(
                &parent.restrictions,
                derived.effective(),
            )
        {
            return Err(GrantRefusal::Denied);
        }
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
        self.apply_change(
            &mut owner,
            &mut publication,
            GrantAuthorityInput::IssueChild {
                actor,
                record: record.clone(),
                derived,
                chain,
                now_ms,
            },
        )
        .map_err(|refusal| issue_refusal(&owner, refusal, &record.id, may_issue))?;
        // Generated rejection precedes every update. Successful issuance is
        // hidden by the actual owner mutex until this publication point. A
        // poison since the reservation leaves it retained but unusable until
        // reconstruction.
        let _ = publication.publish();
        lineage_ref(&owner, &record)
    }

    /// Issue the child of `intent`, or recover it when this owner already
    /// retains exactly that child, inside one non-waiting reservation of the
    /// grant publication, so no owner on the publication (grants, mandates,
    /// source custody) changes between the checks and the append.
    ///
    /// Under the reservation, before the grant-state mutex:
    /// - the parent lineage is resolved now as a controller lineage for the
    ///   issuer and represented subject (the independent parent, current);
    /// - `originating` runs: the caller's further conjunct over its own
    ///   owners (for helper work, every originating source against the
    ///   actual setup operation). It returns the window in which its
    ///   decision holds. It is an additional restriction only, never
    ///   originating authority: without it this is
    ///   [`issue_child`](Self::issue_child)'s own decision.
    ///
    /// Then, under the mutex, the intended id is looked up. A retained
    /// record must be exactly the intended child (parent, issuer, grantee,
    /// subject and effective bounds) and is resolved as a controller lineage
    /// for the grantee, without another append. Otherwise the child is
    /// appended through the held reservation, when its effective bounds are
    /// controller-compatible (an unrestricted lifetime), after a final clock
    /// read inside the conjunct's window. A recovery takes the same final
    /// read. Nothing here waits.
    ///
    /// A poison of the publication raised after the reservation and before
    /// the pre-append check that follows `originating` (always so for
    /// `originating`'s own poison) refuses the call before the grant-state
    /// mutex: nothing is appended. A poison another thread raises after that
    /// check cannot stop the append it follows: that child was decided from
    /// consistent reads, stays unusable until the owners are reconstructed,
    /// and is valid (recovered by the same intent) after. A conjunct that
    /// finds its own entry lock poisoned, or catches a panic, returns `Err`
    /// and never reads through `PoisonError::into_inner`.
    ///
    /// # Errors
    /// [`ChildGrantRefusal::Uncertain`] when the append's outcome is unknown;
    /// recover with the same intent from the restored owner.
    /// [`ChildGrantRefusal::Unavailable`] when the publication was poisoned
    /// after the reservation and before the pre-append check (always so for
    /// `originating`'s own poison), with nothing appended.
    pub fn issue_or_recover_child_under_exclusion(
        &self,
        intent: &ChildGrantIntent,
        originating: impl FnOnce() -> Result<ChildGrantValidity, GrantRefusal>,
    ) -> Result<ChildGrantOutcome, ChildGrantRefusal> {
        // Held once, never awaited, and released only after the append is
        // published or refused: no owner on the publication changes between
        // the checks below and the append.
        let mut publication = self
            .publication
            .try_reserve_owner_change()
            .map_err(|_| ChildGrantRefusal::Unavailable)?;
        self.issue_or_recover_child_reserved(&mut publication, intent, originating)
    }

    /// [`issue_or_recover_child_under_exclusion`](Self::issue_or_recover_child_under_exclusion)
    /// for an owner of this crate that already holds the publication
    /// reservation (`publication`) and runs its own checks under it; the
    /// reservation is not taken again.
    /// Whether `publication` is this grant owner's own publication.
    pub(crate) fn shares_publication(&self, publication: &LocalAuthorizationPublication) -> bool {
        self.publication.same_instance(publication)
    }

    pub(crate) fn issue_or_recover_child_reserved(
        &self,
        publication: &mut crate::publication::LocalPublicationGuard<'_>,
        intent: &ChildGrantIntent,
        originating: impl FnOnce() -> Result<ChildGrantValidity, GrantRefusal>,
    ) -> Result<ChildGrantOutcome, ChildGrantRefusal> {
        let refused = |refusal: GrantRefusal| match refusal {
            GrantRefusal::Unavailable => ChildGrantRefusal::Unavailable,
            GrantRefusal::Denied | GrantRefusal::Duplicate | GrantRefusal::ControllerInUse => {
                ChildGrantRefusal::Denied
            }
        };
        let actor = principal(intent.issuer.clone()).map_err(refused)?;
        let grantee = principal(intent.grantee.clone()).map_err(refused)?;
        let represented_subject = intent
            .represented_subject
            .clone()
            .map(principal)
            .transpose()
            .map_err(refused)?;
        let parent_reference = intent
            .parent_lineage
            .last()
            .ok_or(ChildGrantRefusal::Denied)?;
        // Every resolution runs before the grant-state mutex; each takes it
        // itself. The independent parent must be a current controller
        // lineage of the issuer now; no other lineage stands in for it.
        self.resolve_controller_lineage(
            &intent.parent_lineage,
            &intent.issuer,
            intent.represented_subject.as_ref(),
        )
        .map_err(refused)?;
        let validity = originating().map_err(refused)?;
        // A poison since the reservation refuses before any append. This
        // thread raised any poison of the conjunct's own before this load.
        if !publication.reservation_is_intact() {
            return Err(ChildGrantRefusal::Unavailable);
        }

        let mut owner = self
            .owner
            .lock()
            .map_err(|_| ChildGrantRefusal::Unavailable)?;
        if let Some(durable) = &self.durable {
            durable.check_available().map_err(refused)?;
        }
        let parent = exact_record(&owner, parent_reference)
            .map_err(refused)?
            .clone();
        if parent.grantee != actor || parent.represented_subject != represented_subject {
            return Err(ChildGrantRefusal::Denied);
        }
        let derived =
            DerivedChildRestrictions::new(parent.restrictions.clone(), intent.requested.clone())
                .map_err(|_| ChildGrantRefusal::Denied)?;
        if let Some(retained) = owner.state().records.get(&intent.id) {
            // Only exactly the intended child is recovered; an id naming
            // any other grant is refused, never accepted or replaced.
            if retained.parent.as_ref() != Some(&parent.id)
                || retained.issuer != actor
                || retained.grantee != grantee
                || retained.represented_subject != represented_subject
                || &retained.restrictions != derived.effective()
            {
                return Err(ChildGrantRefusal::Denied);
            }
            let leaf = lineage_ref(&owner, retained).map_err(refused)?;
            drop(owner);
            let mut lineage = intent.parent_lineage.clone();
            lineage.push(leaf);
            // The retained record is identity only: it is usable only as
            // the intended grantee's current controller lineage with the
            // intended effective bounds, after the same current checks.
            let resolved = self
                .resolve_controller_lineage(
                    &lineage,
                    &intent.grantee,
                    intent.represented_subject.as_ref(),
                )
                .map_err(refused)?;
            if resolved.restrictions() != derived.effective() {
                return Err(ChildGrantRefusal::Denied);
            }
            self.require_within(validity)?;
            return Ok(ChildGrantOutcome::Recovered { lineage });
        }
        // The child must be usable as a controller lineage, as a recovery
        // requires: a finite effective lifetime is refused before append.
        if !matches!(
            derived.effective().lifetime.bound(),
            LifetimeBound::Unrestricted
        ) {
            return Err(ChildGrantRefusal::Denied);
        }
        let chain = chain(&owner, &parent).map_err(refused)?;
        let record = GrantRecord {
            id: intent.id.clone(),
            authority_incarnation: owner
                .state()
                .incarnation
                .ok_or(ChildGrantRefusal::Unavailable)?,
            parent: Some(parent.id),
            issuer: actor.clone(),
            grantee,
            represented_subject,
            issued_revision: owner
                .state()
                .revision
                .checked_add(1)
                .ok_or(ChildGrantRefusal::Unavailable)?,
            restrictions: derived.effective().clone(),
        };
        // The final time check immediately before the mutation: the
        // reservation freezes owner changes, not time.
        let now_ms = self.require_within(validity)?;
        if let Err(refusal) = self.apply_change(
            &mut owner,
            publication,
            GrantAuthorityInput::IssueChild {
                actor,
                record: record.clone(),
                derived,
                chain,
                now_ms,
            },
        ) {
            let uncertain = self
                .durable
                .as_ref()
                .is_some_and(|durable| durable.check_available().is_err());
            return Err(if uncertain {
                ChildGrantRefusal::Uncertain
            } else {
                refused(refusal)
            });
        }
        // Generated rejection precedes every update. Successful issuance is
        // hidden by the actual owner mutex until this publication point. A
        // poison since the reservation leaves it retained but unusable until
        // reconstruction.
        let _ = publication.publish();
        let leaf = lineage_ref(&owner, &record).map_err(refused)?;
        let mut lineage = intent.parent_lineage.clone();
        lineage.push(leaf);
        Ok(ChildGrantOutcome::Issued { lineage })
    }

    /// The current wall time, read now, when that read is inside `validity`
    /// on both clocks.
    fn require_within(&self, validity: ChildGrantValidity) -> Result<u64, ChildGrantRefusal> {
        let now = self
            .clock
            .now()
            .map_err(|_| ChildGrantRefusal::Unavailable)?;
        if now.unix_ms < validity.not_before_ms
            || now.unix_ms >= validity.expires_at_ms
            || now.monotonic >= validity.deadline
        {
            return Err(ChildGrantRefusal::Denied);
        }
        Ok(now.unix_ms)
    }

    /// The reference this owner retained for the grant it issued under `id`
    /// (its issued revision and this owner's incarnation), or `None` when it
    /// issued none. A revoked grant keeps its reference.
    ///
    /// The reference proves the retained identity only. It may belong to
    /// another issuer, grantee, parent or restriction set than a caller
    /// intended, and it does not say the grant is usable. To reconcile an
    /// intended issuance as currently usable, append this leaf to the exact
    /// parent lineage retained with the original intent, resolve that
    /// lineage ([`resolve_lineage`](Self::resolve_lineage)) with the
    /// intended executor and represented subject, and compare the resolved
    /// canonical restrictions with the intended ones. The lookup itself
    /// never issues, revives or publishes.
    pub fn issued_reference(
        &self,
        id: &EvidenceId,
    ) -> Result<Option<GrantLineageRef>, GrantRefusal> {
        // The point an availability check before the owner mutex would
        // already have passed; tests acknowledge the lookup here.
        #[cfg(test)]
        test_boundary::reached_lookup();
        let owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        // Checked under the owner mutex: an issuance whose append became
        // uncertain marks custody failed before it releases the mutex, so a
        // lookup waiting behind it never answers from stale memory.
        if let Some(durable) = &self.durable {
            durable.check_available()?;
        }
        owner
            .state()
            .records
            .get(id)
            .map(|record| lineage_ref(&owner, record))
            .transpose()
    }

    /// Revocation never deletes or reuses an issued ID. Either the configured
    /// root or the actual issuer can revoke; an expired ancestor cannot revive it.
    /// Native custody vetoes only references in unfinished controller lineages,
    /// including their ancestors. Ordinary delegation/work-use references do
    /// not prevent revocation. The veto has the distinct `ControllerInUse` result.
    /// A caller who is neither the configured root nor the grant's issuer, or
    /// a reference that is not an exact retained grant, is `Denied` before
    /// any custody or publication reservation is taken, so a busy
    /// reservation never hides that refusal. Nothing waits: a busy or
    /// poisoned grant owner is `Unavailable` for every caller, and once
    /// custody permits mutation, busy or poisoned local publication/grant
    /// locks return `Unavailable` without waiting, mutation or publication.
    pub fn revoke(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
        custody: &mut impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody,
    ) -> Result<(), GrantRefusal> {
        self.revoke_reserved(caller, grant, custody, |_| Ok(()))
    }

    /// [`revoke`](Self::revoke) for a caller that decided against
    /// `expected`, a stamp it observed on this owner's publication. Under
    /// the revocation's own publication reservation, before the owner lock
    /// or any mutation, `expected` must be a stamp of this same publication
    /// and still current. An owner change that published after the caller's
    /// check, or a stamp of another publication, is
    /// [`ExpectedRevokeRefusal::NotCurrent`]: nothing changes and nothing is
    /// published. Every other result is that of [`revoke`](Self::revoke).
    pub fn revoke_expecting(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
        expected: &LocalPublicationStamp,
        custody: &mut impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody,
    ) -> Result<(), ExpectedRevokeRefusal> {
        self.revoke_reserved(caller, grant, custody, |reservation| {
            if reservation.stamp_is_current(expected) {
                Ok(())
            } else {
                Err(ExpectedRevokeRefusal::NotCurrent)
            }
        })
    }

    /// Both revocation entries. `under_reservation` runs once, with the
    /// publication reservation held and before the owner lock or any
    /// mutation; its refusal changes and publishes nothing.
    fn revoke_reserved<E: From<GrantRefusal>>(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
        custody: &mut impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody,
        under_reservation: impl FnOnce(&LocalPublicationGuard<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        use meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal;
        self.check_revoke_authority(caller, grant)?;
        custody
            .with_unreferenced_controller_grant(grant, || {
                self.revoke_under_native_custody(caller, grant, under_reservation)
            })
            .map_err(|refusal| {
                E::from(match refusal {
                    ControllerCustodyRefusal::ControllerInUse => GrantRefusal::ControllerInUse,
                    ControllerCustodyRefusal::Unavailable => GrantRefusal::Unavailable,
                })
            })?
    }

    /// Read-only and without waiting: `caller` may revoke the exact retained
    /// `grant` (the configured root or its issuer). The root and issued
    /// records never change, so this answer cannot go stale; the generated
    /// owner still decides at the mutation itself.
    fn check_revoke_authority(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
    ) -> Result<(), GrantRefusal> {
        let actor = principal(caller.clone())?;
        let owner = self
            .owner
            .try_lock()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let record = exact_record(&owner, grant)?;
        if owner.state().root.as_ref() == Some(&actor) || record.issuer == actor {
            Ok(())
        } else {
            Err(GrantRefusal::Denied)
        }
    }

    fn revoke_under_native_custody<E: From<GrantRefusal>>(
        &self,
        caller: &PrincipalRef,
        grant: &GrantLineageRef,
        under_reservation: impl FnOnce(&LocalPublicationGuard<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        let actor = principal(caller.clone())?;
        let mut publication = self
            .publication
            .try_reserve_owner_change()
            .map_err(|_| GrantRefusal::Unavailable)?;
        // No other owner change on this publication can publish while the
        // reservation is held, so what holds here still holds at the apply.
        under_reservation(&publication)?;
        let mut owner = self
            .owner
            .try_lock()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let record = exact_record(&owner, grant)?.clone();
        let revision = owner.state().revision;
        self.apply_change(
            &mut owner,
            &mut publication,
            GrantAuthorityInput::Revoke { actor, record },
        )?;
        // RevokeAlready is a canonical successful no-op. Only RevokeNew
        // advances this owner's revision, so no second semantic decision is
        // made here and repeated revocation does not churn observations.
        if owner.state().revision != revision {
            // A poison since the reservation leaves the revocation retained,
            // through reconstruction, with every check unavailable meanwhile.
            let _ = publication.publish();
        }
        Ok(())
    }

    /// Apply one accepted administrative change. With durable custody the
    /// change is acknowledged by the store before it is installed; callers
    /// publish the change only after this returns `Ok`.
    fn apply_change(
        &self,
        owner: &mut GrantAuthorityMachineAuthority,
        publication: &mut crate::publication::LocalPublicationGuard<'_>,
        input: GrantAuthorityInput,
    ) -> Result<(), GrantRefusal> {
        match &self.durable {
            Some(durable) => durable.apply(owner, publication, input),
            None => owner
                .apply(input)
                .map(|_| ())
                .map_err(|_| GrantRefusal::Denied),
        }
    }

    /// Run `operation` while every owner change on this grant authority's
    /// publication is excluded, without publishing one: grants, mandates and
    /// source-original custody that share the publication cannot change
    /// until it returns. Never waits; contention or an unavailable
    /// publication refuses before `operation` runs.
    ///
    /// For a host commit gated on current native facts (for example a
    /// WorkGraph admission fence). `operation` must not mutate any owner on
    /// this publication and must not await.
    ///
    /// # Errors
    /// [`PublicationError::Unavailable`] when the publication is held,
    /// poisoned or exhausted.
    pub fn try_with_owner_changes_excluded<T>(
        &self,
        operation: impl FnOnce() -> T,
    ) -> Result<T, PublicationError> {
        // Held, never published: dropping the reservation leaves the
        // publication sequence unchanged.
        let _reservation = self.publication.try_reserve_owner_change()?;
        Ok(operation())
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
        self.resolve_lineage_for_use(
            references,
            LineageUse::Executor {
                executor,
                represented_subject,
            },
            false,
        )
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
        self.resolve_lineage_for_use(
            references,
            LineageUse::Executor {
                executor,
                represented_subject,
            },
            true,
        )
    }

    /// Liveness only: whether the exact root-to-leaf `references` are, now,
    /// a current lineage of this owner for the retained leaf's own grantee
    /// and represented subject. That identity is read from the leaf under
    /// the same owner lock and publication observation as the generated
    /// use; no caller supplies it. Every other use check applies unchanged:
    /// this owner's namespace, generation and incarnation, the exact chain
    /// links, revocation and every link's current lifetime.
    ///
    /// It returns no grant. The answer permits no operation and names no
    /// executor; a caller authorizing an operation resolves the lineage for
    /// its actual executor ([`resolve_lineage`](Self::resolve_lineage)).
    ///
    /// # Errors
    /// [`GrantRefusal::Denied`] when the lineage is not current;
    /// [`GrantRefusal::Unavailable`] when this owner cannot say now.
    pub fn require_live_lineage(&self, references: &[GrantLineageRef]) -> Result<(), GrantRefusal> {
        self.resolve_lineage_for_use(references, LineageUse::LeafOwn, false)
            .map(|_| ())
    }

    /// The ceiling the exact root-to-leaf `references` impose now: the
    /// lineage resolved for the retained leaf's own grantee and represented
    /// subject, in the same single owner observation as
    /// [`require_live_lineage`](Self::require_live_lineage), with every
    /// other use check unchanged.
    ///
    /// A ceiling only narrows: its restrictions and expiry are conjoined
    /// with an operation's own authority. It is returned as a
    /// [`ContributorCeiling`], never as a [`ResolvedGrant`]: it is never an
    /// executor's authority, which that executor's own lineage supplies
    /// ([`resolve_lineage`](Self::resolve_lineage)).
    ///
    /// # Errors
    /// [`GrantRefusal::Denied`] when the lineage is not current;
    /// [`GrantRefusal::Unavailable`] when this owner cannot say now.
    pub fn resolve_lineage_ceiling(
        &self,
        references: &[GrantLineageRef],
    ) -> Result<ContributorCeiling, GrantRefusal> {
        self.resolve_lineage_ceiling_observed(references)
            .map_err(standalone_refusal)
    }

    /// [`resolve_lineage`](Self::resolve_lineage) for a caller inside its
    /// own enclosing observation of this owner's publication: an owner
    /// change published during the resolution is
    /// [`LineageResolutionRefusal::Changed`], which that caller classifies
    /// as a stale read, not as unavailability.
    ///
    /// # Errors
    /// [`LineageResolutionRefusal::Changed`] for such a change; otherwise
    /// [`LineageResolutionRefusal::Refused`] with the refusal
    /// [`resolve_lineage`](Self::resolve_lineage) reports.
    pub fn resolve_lineage_observed(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
    ) -> Result<ResolvedGrant, LineageResolutionRefusal> {
        self.resolve_lineage_observed_for_use(
            references,
            LineageUse::Executor {
                executor,
                represented_subject,
            },
            false,
        )
    }

    /// [`resolve_controller_lineage`](Self::resolve_controller_lineage)
    /// for a caller inside its own enclosing observation; see
    /// [`resolve_lineage_observed`](Self::resolve_lineage_observed).
    ///
    /// # Errors
    /// As [`resolve_lineage_observed`](Self::resolve_lineage_observed).
    pub fn resolve_controller_lineage_observed(
        &self,
        references: &[GrantLineageRef],
        executor: &PrincipalRef,
        represented_subject: Option<&PrincipalRef>,
    ) -> Result<ResolvedGrant, LineageResolutionRefusal> {
        self.resolve_lineage_observed_for_use(
            references,
            LineageUse::Executor {
                executor,
                represented_subject,
            },
            true,
        )
    }

    /// [`resolve_lineage_ceiling`](Self::resolve_lineage_ceiling) for a
    /// caller inside its own enclosing observation; see
    /// [`resolve_lineage_observed`](Self::resolve_lineage_observed).
    ///
    /// # Errors
    /// As [`resolve_lineage_observed`](Self::resolve_lineage_observed).
    pub fn resolve_lineage_ceiling_observed(
        &self,
        references: &[GrantLineageRef],
    ) -> Result<ContributorCeiling, LineageResolutionRefusal> {
        // The lineage restrictions stay here: a ceiling carries no evidence.
        self.resolve_lineage_observed_for_use(references, LineageUse::LeafOwn, false)
            .map(|resolved| ContributorCeiling {
                restrictions: resolved.restrictions,
                expires_at_ms: resolved.expires_at_ms,
                publication: resolved.publication,
            })
    }

    fn resolve_lineage_for_use(
        &self,
        references: &[GrantLineageRef],
        lineage_use: LineageUse<'_>,
        controller: bool,
    ) -> Result<ResolvedGrant, GrantRefusal> {
        self.resolve_lineage_observed_for_use(references, lineage_use, controller)
            .map_err(standalone_refusal)
    }

    fn resolve_lineage_observed_for_use(
        &self,
        references: &[GrantLineageRef],
        lineage_use: LineageUse<'_>,
        controller: bool,
    ) -> Result<ResolvedGrant, LineageResolutionRefusal> {
        let (resolved, publication) = self
            .publication
            .observe(|| self.resolve_lineage_data(references, lineage_use, controller))
            .map_err(|error| match error {
                PublicationError::Changed => LineageResolutionRefusal::Changed,
                PublicationError::Unavailable => {
                    LineageResolutionRefusal::Refused(GrantRefusal::Unavailable)
                }
            })?;
        let (restrictions, lineage, expires_at_ms) =
            resolved.map_err(LineageResolutionRefusal::Refused)?;
        Ok(ResolvedGrant {
            restrictions,
            lineage,
            expires_at_ms,
            publication,
        })
    }

    // Called only inside the actual publication observation above. Resolution
    // prepares a disposable view; it does not run on the warm entry-check path.
    fn resolve_lineage_data(
        &self,
        references: &[GrantLineageRef],
        lineage_use: LineageUse<'_>,
        controller: bool,
    ) -> Result<(ExecutionRestrictions, Vec<ExecutionRestrictions>, u64), GrantRefusal> {
        if let Some(durable) = &self.durable {
            durable.check_available()?;
        }
        let claimed = match lineage_use {
            LineageUse::Executor {
                executor,
                represented_subject,
            } => Some((
                principal(executor.clone())?,
                represented_subject.cloned().map(principal).transpose()?,
            )),
            LineageUse::LeafOwn => None,
        };
        let now_ms = self
            .clock
            .now()
            .map_err(|_| GrantRefusal::Unavailable)?
            .unix_ms;
        let mut owner = self.owner.lock().map_err(|_| GrantRefusal::Unavailable)?;
        let reference = references.last().ok_or(GrantRefusal::Denied)?;
        let leaf = exact_record(&owner, reference)?.clone();
        // A leaf-own use (liveness or a ceiling) names the retained leaf's
        // own identity, read here, under the same owner lock as the
        // generated use below.
        let (executor, represented_subject) =
            claimed.unwrap_or_else(|| (leaf.grantee.clone(), leaf.represented_subject.clone()));
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
        let lineage = chain
            .iter()
            .map(|record| record.restrictions.clone())
            .collect::<Vec<_>>();
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
        Ok((restrictions, lineage, expires_at_ms))
    }
}

/// Whose use of a lineage the generated owner checks.
#[derive(Clone, Copy)]
enum LineageUse<'a> {
    /// The caller's executor and represented subject: separate facts the
    /// leaf's grantee and represented subject must equal.
    Executor {
        executor: &'a PrincipalRef,
        represented_subject: Option<&'a PrincipalRef>,
    },
    /// The retained leaf's own grantee and represented subject, read under
    /// the same owner lock as the use: a liveness check or a ceiling, never
    /// an executor's authority.
    LeafOwn,
}

/// A generated rejection of an issuance the caller may perform, for an id
/// this owner already retains, is the typed duplicate. Every other refusal,
/// including any refusal of a caller who may not issue there, is unchanged.
fn issue_refusal(
    owner: &GrantAuthorityMachineAuthority,
    refusal: GrantRefusal,
    id: &EvidenceId,
    may_issue: bool,
) -> GrantRefusal {
    if refusal == GrantRefusal::Denied && may_issue && owner.state().records.contains_key(id) {
        GrantRefusal::Duplicate
    } else {
        refusal
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

/// Test-only acknowledgement points, per thread: a test installs a hook on
/// the thread that will make the call.
#[cfg(test)]
pub(crate) mod test_boundary {
    use std::cell::RefCell;

    type Hook = Box<dyn FnOnce()>;

    thread_local! {
        static LOOKUP: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Run `hook` when this thread's next lookup reaches the owner mutex.
    pub(crate) fn on_lookup(hook: impl FnOnce() + 'static) {
        LOOKUP.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    pub(super) fn reached_lookup() {
        if let Some(hook) = LOOKUP.with(|slot| slot.borrow_mut().take()) {
            hook();
        }
    }
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
mod durable_tests;
#[cfg(test)]
mod reconstruction_tests;
