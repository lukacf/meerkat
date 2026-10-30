//! Rebuildable per-work local authorization and constant-time final checks.

use std::sync::Arc;

use meerkat_authorization_contracts::constraints::LifetimeBound;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::authorization::OperationReviewTier;
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationUse, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, ReviewOperationContextResolution,
    WorkAuthorization,
};
use meerkat_core::time_compat::{Duration, Instant};

/// Whether a model operation is the admitted controller's own route, decided
/// under the controller purpose (controller lineage C) rather than as an
/// ordinary operation: the controller's inference, and the housekeeping
/// compaction of its own context when it runs on exactly the admitted
/// controller selection, with no hosted capability and no live channel. A
/// `Compaction` label on any other route grants nothing here; it stays an
/// operation under the work's operation authority.
pub(crate) fn controller_route(
    facts: &meerkat_core::authorization::ModelAuthorizationFacts,
    controller: Option<&meerkat_core::ControllerModelSelection>,
) -> bool {
    match facts.usage {
        ModelAuthorizationUse::ControllerInference => true,
        ModelAuthorizationUse::Compaction => {
            facts.hosted_capabilities.is_empty()
                && facts.live_channel.is_none()
                && controller.is_some_and(|selection| selection.matches_model_facts(facts))
        }
        _ => false,
    }
}

use crate::policy::{LocalPolicyAllowance, LocalPolicyPurpose, LocalWorkPolicy};
use crate::publication::{LocalAuthorizationPublication, LocalPublicationStamp, PublicationError};

pub use crate::clock::{
    HostAuthorizationClock, LocalAuthorizationClock, LocalAuthorizationTime, LocalClockError,
};

/// Host-installed composition for one immutable native work association.
///
/// The policy owner must verify that association against real admitted work on
/// every preparation. Constructing this handle is not acceptance of a decoded
/// claim. Never persist it or keep it as a session-wide default. All policy-owner
/// mutations must share the same publication instance for this handle's lifetime.
pub struct LocalWorkAuthorization {
    associations: Arc<[InputAuthorityAssociation]>,
    policy: Arc<dyn LocalWorkPolicy>,
    publication: LocalAuthorizationPublication,
    clock: Arc<dyn LocalAuthorizationClock>,
}

impl LocalWorkAuthorization {
    #[must_use]
    pub fn new(
        association: Arc<InputAuthorityAssociation>,
        policy: Arc<dyn LocalWorkPolicy>,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
    ) -> Self {
        Self {
            associations: vec![association.as_ref().clone()].into(),
            policy,
            publication,
            clock,
        }
    }

    /// Compose every retained native contributor, without electing the first
    /// contributor's mandate or unioning permissions. This validates data shape
    /// only; the policy still resolves every original input on preparation.
    pub fn new_batch(
        associations: Arc<[InputAuthorityAssociation]>,
        policy: Arc<dyn LocalWorkPolicy>,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
    ) -> Result<Self, OperationRefused> {
        let first = associations.first().ok_or_else(denied)?;
        if associations
            .iter()
            .any(|other| !first.batch_compatible_with(other))
        {
            return Err(denied());
        }
        Ok(Self {
            associations,
            policy,
            publication,
            clock,
        })
    }

    fn compile(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<CompiledLocalAuthorization, meerkat_core::OperationAuthorizationError> {
        self.compile_within(binding, &[])
    }

    /// The complete prepared-entry decision for `binding` (every association
    /// and purpose, admitted ceilings, review tier, window and deadline),
    /// with `further` grants conjoined into each allowance's restrictions
    /// and expiry before its operation values are bounded.
    fn compile_within(
        &self,
        binding: &PreparedAuthorizationBinding,
        further: &[crate::grants::ResolvedGrant],
    ) -> Result<CompiledLocalAuthorization, meerkat_core::OperationAuthorizationError> {
        let now = self
            .clock
            .now()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        // All live policy facts are read within this bracket. An old policy
        // view cannot acquire a new publication stamp.
        let mut known_failure = None;
        let observed = self.publication.observe(|| {
            let result = (|| -> Result<_, meerkat_core::OperationAuthorizationError> {
                let mut combined: Option<(u64, u64, Instant, OperationReviewTier)> = None;
                // A batch shares one admitted controller selection.
                let controller = self
                    .associations
                    .first()
                    .and_then(|association| association.candidate().controller_model.as_ref());
                let purposes: &[LocalPolicyPurpose] = match &binding.facts().operation {
                    AuthorizationOperation::Model(facts) if controller_route(facts, controller) => {
                        if facts.hosted_capabilities.is_empty() {
                            &[LocalPolicyPurpose::Controller]
                        } else {
                            &[
                                LocalPolicyPurpose::Controller,
                                LocalPolicyPurpose::Operation,
                            ]
                        }
                    }
                    _ => &[LocalPolicyPurpose::Operation],
                };
                for association in self.associations.iter() {
                    for &purpose in purposes {
                        let allowance =
                            self.policy
                                .evaluate_for(association, binding, purpose, now.unix_ms)?;
                        let current = self.validate_allowance(
                            association,
                            binding,
                            purpose,
                            allowance,
                            further,
                            now,
                        )?;
                        combined = Some(match combined {
                            None => current,
                            // Every contributor's owner tier applies; keep
                            // the strictest.
                            Some(previous) => (
                                previous.0.max(current.0),
                                previous.1.min(current.1),
                                previous.2.min(current.2),
                                previous.3.max(current.3),
                            ),
                        });
                    }
                }
                combined.ok_or_else(denied).map_err(Into::into)
            })();
            if matches!(
                result,
                Err(meerkat_core::OperationAuthorizationError::Unavailable
                    | meerkat_core::OperationAuthorizationError::ObservationUnavailable(_))
            ) {
                known_failure = result.as_ref().err().copied();
            }
            result
        });
        if let Some(error) = known_failure {
            return Err(error);
        }
        let (prepared, publication) = observed.map_err(publication_refusal)?;
        let (not_before_ms, expires_at_ms, deadline, review_tier) = prepared?;
        Ok(CompiledLocalAuthorization {
            binding: binding.clone(),
            publication,
            clock: Arc::clone(&self.clock),
            not_before_ms,
            expires_at_ms,
            deadline,
            review_tier,
        })
    }

    fn validate_allowance(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        mut allowance: LocalPolicyAllowance,
        further: &[crate::grants::ResolvedGrant],
        now: LocalAuthorizationTime,
    ) -> Result<(u64, u64, Instant, OperationReviewTier), OperationRefused> {
        for grant in further {
            allowance.restrictions = allowance.restrictions.conjoin(grant.restrictions());
            allowance.expires_at_ms = allowance.expires_at_ms.min(grant.expires_at_ms());
        }
        if allowance.operation_values.is_empty() || allowance.expires_at_ms <= now.unix_ms {
            return Err(denied());
        }
        let candidate = association.candidate();
        // Controller and hosted-operation duties are conjunctive, but each
        // duty has its own correlated values and retained ceiling. Applying
        // the controller's infer-only ceiling to web_search would incorrectly
        // disable a separately authorized hosted capability.
        let ceiling = match purpose {
            LocalPolicyPurpose::Controller => {
                let AuthorizationOperation::Model(facts) = &binding.facts().operation else {
                    return Err(denied());
                };
                if !controller_route(facts, candidate.controller_model.as_ref())
                    || !candidate
                        .controller_model
                        .as_ref()
                        .is_some_and(|selection| selection.matches_model_facts(facts))
                {
                    return Err(denied());
                }
                &candidate.controller_ceiling
            }
            LocalPolicyPurpose::Operation => &candidate.admitted_ceiling,
        };
        let restrictions = ceiling.conjoin(&allowance.restrictions);
        let (not_before_ms, expires_at_ms) = match restrictions.lifetime.bound() {
            LifetimeBound::Unrestricted => (0, allowance.expires_at_ms),
            LifetimeBound::Window {
                not_before_ms,
                expires_at_ms,
            } => (not_before_ms, expires_at_ms.min(allowance.expires_at_ms)),
            LifetimeBound::Empty => return Err(denied()),
        };
        for values in &allowance.operation_values {
            restrictions
                .check_bounds(values.at(now.unix_ms))
                .map_err(|_| denied())?;
        }
        let remaining_ms = expires_at_ms
            .checked_sub(now.unix_ms)
            .filter(|ms| *ms > 0)
            .ok_or_else(denied)?;
        let deadline = now
            .monotonic
            .checked_add(Duration::from_millis(remaining_ms))
            .ok_or_else(denied)?;
        Ok((
            not_before_ms,
            expires_at_ms,
            deadline,
            allowance.review_tier,
        ))
    }
}

/// The source owner's view of one original, for further-original
/// conjunction: its current retained mandate when it is active and still
/// retained (`Ok(None)`: not retained). A cancelled original refuses. It is
/// never handed an operation or its payload. Synchronous, no I/O, and no
/// native lock: it is read under the work owner's exclusion.
pub trait OriginalBasisSource: Send + Sync {
    /// The original is not operation-cancelled by its source owner. Used for
    /// the work's own originals, whose mandate is the association's own.
    fn require_active(
        &self,
        namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        original: &meerkat_authorization_contracts::work_association::OriginalWorkRef,
    ) -> Result<(), meerkat_core::OperationAuthorizationError>;

    fn active_retained_basis(
        &self,
        namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        original: &meerkat_authorization_contracts::work_association::OriginalWorkRef,
    ) -> Result<
        Option<meerkat_authorization_contracts::work_association::WorkAuthorityBasis>,
        meerkat_core::OperationAuthorizationError,
    >;
}

/// One original as its source owner holds it: the ingress namespace it was
/// admitted in and its work reference. The same reference under another
/// ingress is another original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedOriginal {
    pub namespace: meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
    pub original: meerkat_authorization_contracts::work_association::OriginalWorkRef,
}

impl RetainedOriginal {
    /// The same original: the same work reference under the same realm and
    /// ingress (the occurrence scope does not distinguish originals).
    pub fn same_original(&self, other: &Self) -> bool {
        self.original == other.original
            && self.namespace.realm == other.namespace.realm
            && self.namespace.ingress == other.namespace.ingress
    }
}

/// Further retained originals a trusted native composition conjoins with a
/// work's own operation (`WorkAuthorization::run_if_current_within`): each
/// original's own grant for the operation's target bounds it, read from
/// `source` under the original's own ingress namespace.
pub struct FurtherOriginals {
    pub originals: Arc<[RetainedOriginal]>,
    pub source: Arc<dyn OriginalBasisSource>,
}

impl LocalWorkAuthorization {
    /// The typed child-issuance inputs and this owner's issuance
    /// composition: the intent is a `ChildGrantIntent` (anything else is
    /// refused), `further` is `FurtherOriginals`, and the grant-backed
    /// policy's grant owner shares this work's publication (its reserved
    /// append runs under this work's reservation).
    #[allow(clippy::type_complexity)]
    fn child_issuance_inputs<'a>(
        &self,
        further: &'a (dyn std::any::Any + Send + Sync),
        intent: &'a (dyn std::any::Any + Send + Sync),
    ) -> Result<
        (
            &'a crate::grants::ChildGrantIntent,
            &'a FurtherOriginals,
            Arc<crate::grant_policy::GrantBackedWorkPolicy>,
        ),
        meerkat_core::OperationAuthorizationError,
    > {
        let intent = intent
            .downcast_ref::<crate::grants::ChildGrantIntent>()
            .ok_or_else(denied)?;
        let further = further
            .downcast_ref::<FurtherOriginals>()
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        let policy = Arc::clone(&self.policy)
            .grant_backed()
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        if !policy.grants().shares_publication(&self.publication) {
            return Err(meerkat_core::OperationAuthorizationError::Unavailable);
        }
        Ok((intent, further, policy))
    }

    /// The fresh decision's window and monotonic cutoff intersected with
    /// the validity of the decision the caller actually holds (latest start,
    /// earliest expiry, earliest deadline), required to hold now. A caller
    /// without a reported validity refuses: the fresh decision alone never
    /// stands in for it.
    fn within_prepared(
        &self,
        decision: &CompiledLocalAuthorization,
        prepared: Option<&meerkat_core::authorization::OperationValidity>,
    ) -> Result<crate::grants::ChildGrantValidity, meerkat_core::OperationAuthorizationError> {
        let prepared = prepared.ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        let validity = crate::grants::ChildGrantValidity {
            not_before_ms: decision.not_before_ms.max(prepared.not_before_ms),
            expires_at_ms: decision.expires_at_ms.min(prepared.expires_at_ms),
            deadline: decision.deadline.min(prepared.deadline),
        };
        let now = self
            .clock
            .now()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        if now.unix_ms < validity.not_before_ms
            || now.unix_ms >= validity.expires_at_ms
            || now.monotonic >= validity.deadline
        {
            return Err(denied().into());
        }
        Ok(validity)
    }

    /// Run `operation` once while the complete current decision for
    /// `binding`, with `further` conjoined, holds inside `prepared` and
    /// requires no more review than `max_tier`, under this work's publication
    /// reservation held through `operation` (never published: every owner
    /// change on this publication is excluded, and contention refuses).
    fn run_current_within_tier(
        &self,
        binding: &PreparedAuthorizationBinding,
        prepared: Option<&meerkat_core::authorization::OperationValidity>,
        further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
        max_tier: OperationReviewTier,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        let further = further
            .downcast_ref::<FurtherOriginals>()
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        let policy = Arc::clone(&self.policy)
            .grant_backed()
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        let _reservation = self
            .publication
            .try_reserve_owner_change()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        let decision = self.current_decision_within(binding, further, &policy)?;
        self.within_prepared(&decision, prepared)?;
        if decision.review_tier > max_tier {
            return Err(denied().into());
        }
        operation();
        Ok(())
    }

    /// The complete current prepared-entry decision for `binding`, with
    /// each relevant original's own grant for the operation conjoined. The
    /// caller holds this work's publication reservation.
    fn current_decision_within(
        &self,
        binding: &PreparedAuthorizationBinding,
        further: &FurtherOriginals,
        policy: &crate::grant_policy::GrantBackedWorkPolicy,
    ) -> Result<CompiledLocalAuthorization, meerkat_core::OperationAuthorizationError> {
        use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
        let first = self.associations.first().ok_or_else(denied)?;
        let claims = first.candidate();
        let (own, named) = self.relevant_originals(&further.originals);
        for original in &own {
            further
                .source
                .require_active(&original.namespace, &original.original)?;
        }
        let mut grants = Vec::new();
        for retained in named {
            let basis = further
                .source
                .active_retained_basis(&retained.namespace, &retained.original)?
                .ok_or_else(denied)?;
            let lineage = match &basis {
                WorkAuthorityBasis::HostPolicy { .. }
                | WorkAuthorityBasis::ServiceMandate { .. } => {
                    continue;
                }
                WorkAuthorityBasis::GrantLineage { lineage } => lineage.as_slice(),
                WorkAuthorityBasis::TargetedGrantLineages { .. } => {
                    let target = crate::grant_policy::operation_grant_target(binding)?;
                    basis.lineage_for(target.as_ref()).ok_or_else(denied)?
                }
            };
            grants.push(
                policy
                    .grants()
                    .resolve_lineage(
                        lineage,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref(),
                    )
                    .map_err(|refusal| match refusal {
                        crate::grants::GrantRefusal::Denied => {
                            meerkat_core::OperationAuthorizationError::from(denied())
                        }
                        _ => meerkat_core::OperationAuthorizationError::Unavailable,
                    })?,
            );
        }
        // The complete current prepared-entry decision, inside the caller's
        // exclusion: every association's allowance with the further grants
        // conjoined, admitted ceilings, review tier, window and deadline.
        let decision = self.compile_within(binding, &grants)?;
        decision.check_current(binding)?;
        Ok(decision)
    }

    /// Every relevant original of this work, each under its own ingress,
    /// without duplicates: the associations' own originals (their mandate
    /// is the association's own, so only their disposition is read), then
    /// every contributor and `further` original (whose own retained mandate
    /// bounds the operation).
    fn relevant_originals(
        &self,
        further: &[RetainedOriginal],
    ) -> (Vec<RetainedOriginal>, Vec<RetainedOriginal>) {
        let mut own: Vec<RetainedOriginal> = Vec::new();
        let mut named: Vec<RetainedOriginal> = Vec::new();
        for claims in self
            .associations
            .iter()
            .map(InputAuthorityAssociation::candidate)
        {
            let original = RetainedOriginal {
                namespace: claims.ingress_namespace.clone(),
                original: claims.original_work.clone(),
            };
            if !own.iter().any(|known| known.same_original(&original)) {
                own.push(original);
            }
        }
        let contributors = self.associations.iter().flat_map(|association| {
            let claims = association.candidate();
            claims
                .contributing_work
                .iter()
                .map(move |original| RetainedOriginal {
                    namespace: claims.ingress_namespace.clone(),
                    original: original.clone(),
                })
        });
        for original in contributors.chain(further.iter().cloned()) {
            if !own
                .iter()
                .chain(&named)
                .any(|known| known.same_original(&original))
            {
                named.push(original);
            }
        }
        (own, named)
    }
}

impl WorkAuthorization for LocalWorkAuthorization {
    /// A published owner change in progress on this work's publication, or
    /// else a reservation of its writer held now (an owner change being
    /// prepared, a held exclusion or another operation's final entry): a
    /// holder that a stale read or a refusal to rebuild may have met. A
    /// waiting hint only; [`LocalAuthorizationPublication::change_in_progress`]
    /// still names published changes alone.
    fn owner_change_in_progress(
        &self,
    ) -> Option<meerkat_core::authorization::OwnerChangeInProgress> {
        self.publication.change_or_reservation_in_progress()
    }

    fn run_if_current_within(
        &self,
        binding: &PreparedAuthorizationBinding,
        prepared: Option<&meerkat_core::authorization::OperationValidity>,
        further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        // No review can run here: only an R1 entry runs the operation.
        self.run_current_within_tier(
            binding,
            prepared,
            further,
            operation,
            OperationReviewTier::R1,
        )
    }

    fn run_entered_if_current_within(
        &self,
        binding: &PreparedAuthorizationBinding,
        prepared: Option<&meerkat_core::authorization::OperationValidity>,
        further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
        entered_tier: OperationReviewTier,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        // A further step of an entered effect: no review runs here, and the
        // decision may require no more review than the dispatch entered (the
        // issuance's rule).
        self.run_current_within_tier(binding, prepared, further, operation, entered_tier)
    }

    fn validate_child_issuance(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.child_issuance_inputs(further, intent).map(|_| ())
    }

    fn issue_child_if_current_within(
        &self,
        binding: &PreparedAuthorizationBinding,
        prepared: Option<&meerkat_core::authorization::OperationValidity>,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
        entered_tier: OperationReviewTier,
    ) -> Result<Box<dyn std::any::Any + Send>, meerkat_core::OperationAuthorizationError> {
        let (intent, further, policy) = self.child_issuance_inputs(further, intent)?;
        let mut reservation = self
            .publication
            .try_reserve_owner_change()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        let decision = self.current_decision_within(binding, further, &policy)?;
        // The decision may require no more review than this dispatch entered.
        if decision.review_tier > entered_tier {
            return Err(denied().into());
        }
        let validity = self.within_prepared(&decision, prepared)?;
        let outcome =
            policy
                .grants()
                .issue_or_recover_child_reserved(&mut reservation, intent, || Ok(validity));
        Ok(Box::new(outcome))
    }

    /// Every contributor's owner is asked under one publication observation.
    /// A field every input states identically is presented once. Compatible
    /// inputs can hold different material (their own grants and admitted
    /// ceilings), or none, and each of them bounds the operation, so such a
    /// field lists every input's own material (`inputs_review_material`): no
    /// input stands in for another, nothing is unioned, and a field over its
    /// bound is unavailable.
    fn review_operation_context(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, meerkat_core::OperationAuthorizationError> {
        let now = self
            .clock
            .now()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        let mut known_failure = None;
        let observed = self.publication.observe(|| {
            let result = (|| {
                let resolutions = self
                    .associations
                    .iter()
                    .map(|association| {
                        self.policy
                            .review_context(association, binding, now.unix_ms)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                inputs_review_material(&self.associations, &resolutions)
            })();
            if matches!(
                result,
                Err(meerkat_core::OperationAuthorizationError::Unavailable
                    | meerkat_core::OperationAuthorizationError::ObservationUnavailable(_))
            ) {
                known_failure = result.as_ref().err().copied();
            }
            result
        });
        if let Some(error) = known_failure {
            return Err(error);
        }
        let (resolution, _publication) = observed.map_err(publication_refusal)?;
        resolution
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.associations
            .first()?
            .candidate()
            .controller_model
            .clone()
    }

    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        // Exactly one bounded re-observation on a concurrent local publication.
        // A denied/malformed operation is not automatically retried.
        let first = self.compile(binding);
        let prepared = match first {
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::ReprepareRequired =>
            {
                self.compile(binding)?
            }
            result => result?,
        };
        prepared.check_current(binding)?;
        Ok(Arc::new(prepared))
    }
}

/// The review material of a work's inputs, from each input's own owner
/// answer, in input order. Not applicable only when no input has any
/// material. A field every input states identically (every input's value,
/// stated or absent, the same) is presented once. Otherwise that field
/// lists, under the conjunction it states, every input's own value (or
/// "none", including for an input whose material is not applicable),
/// associated with the input's position and its original work in
/// serialized form. The native bounds stay the conjunction of every
/// input's; nothing here is chosen or unioned.
///
/// # Errors
/// A field over [`meerkat_core::authorization::MAX_REVIEW_OPERATION_CONTEXT_BYTES`],
/// or an original that cannot be serialized, is unavailable.
fn inputs_review_material(
    associations: &[InputAuthorityAssociation],
    resolutions: &[ReviewOperationContextResolution],
) -> Result<ReviewOperationContextResolution, meerkat_core::OperationAuthorizationError> {
    use meerkat_core::authorization::ReviewOperationContext;
    use std::fmt::Write as _;
    let materials: Vec<Option<&ReviewOperationContext>> = resolutions
        .iter()
        .map(|resolution| match resolution {
            ReviewOperationContextResolution::NotApplicable => None,
            ReviewOperationContextResolution::Material(context) => Some(context),
        })
        .collect();
    if materials.iter().all(Option::is_none) {
        return Ok(ReviewOperationContextResolution::NotApplicable);
    }
    let field = |read: fn(&ReviewOperationContext) -> Option<&str>,
                 heading: String|
     -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        let values: Vec<Option<&str>> = materials
            .iter()
            .copied()
            .map(|material| material.and_then(read))
            .collect();
        if values.windows(2).all(|pair| pair[0] == pair[1]) {
            return Ok(values.first().copied().flatten().map(str::to_owned));
        }
        let mut listed = heading;
        for (position, (association, value)) in associations.iter().zip(&values).enumerate() {
            let original = serde_json::to_string(&association.candidate().original_work)
                .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
            let _ = write!(
                listed,
                "\nInput {} (original work {original}): {}",
                position + 1,
                value.unwrap_or("none")
            );
        }
        Ok(Some(listed))
    };
    let inputs = associations.len();
    let action_account = field(
        ReviewOperationContext::action_account,
        format!("The {inputs} inputs of this work name their own action accounts:"),
    )?;
    let mandate = field(
        ReviewOperationContext::mandate,
        format!(
            "The {inputs} inputs of this work hold their own mandates, and the operation must \
             be within every one of them:"
        ),
    )?;
    ReviewOperationContext::new(action_account, mandate)
        .map(ReviewOperationContextResolution::Material)
        .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)
}

/// Use the same actual publication, local clock and exact-entry check as
/// admitted work, while retaining non-input control semantics. Each distinct
/// source/destination rule keeps its correlated values and restrictions.
pub(crate) fn compile_context_control(
    publication: &LocalAuthorizationPublication,
    clock: &Arc<dyn LocalAuthorizationClock>,
    binding: &PreparedAuthorizationBinding,
    evaluate: impl FnOnce(
        u64,
    ) -> Result<
        Vec<LocalPolicyAllowance>,
        meerkat_core::OperationAuthorizationError,
    >,
) -> meerkat_core::authorization::ObservedAuthorizationResult<Arc<dyn PreparedOperationAuthorization>>
{
    use meerkat_core::authorization::ObservedAuthorizationResult;
    let now = match clock.now() {
        Ok(now) => now,
        Err(_) => {
            return ObservedAuthorizationResult::unobserved(Err(
                meerkat_core::OperationAuthorizationError::Unavailable,
            ));
        }
    };
    let mut known_failure = None;
    let observed = publication.observe(|| {
        let result = (|| -> Result<_, meerkat_core::OperationAuthorizationError> {
            let allowances = evaluate(now.unix_ms)?;
            if allowances.is_empty() {
                return Err(denied().into());
            }
            let mut not_before = 0;
            let mut expires = u64::MAX;
            let mut review_tier = OperationReviewTier::R1;
            for allowance in allowances {
                if allowance.operation_values.is_empty() || allowance.expires_at_ms <= now.unix_ms {
                    return Err(denied().into());
                }
                let (start, end) = match allowance.restrictions.lifetime.bound() {
                    LifetimeBound::Unrestricted => (0, allowance.expires_at_ms),
                    LifetimeBound::Window {
                        not_before_ms,
                        expires_at_ms,
                    } => (not_before_ms, expires_at_ms.min(allowance.expires_at_ms)),
                    LifetimeBound::Empty => return Err(denied().into()),
                };
                for values in &allowance.operation_values {
                    allowance
                        .restrictions
                        .check_bounds(values.at(now.unix_ms))
                        .map_err(|_| denied())?;
                }
                not_before = not_before.max(start);
                expires = expires.min(end);
                review_tier = review_tier.max(allowance.review_tier);
            }
            let remaining = expires
                .checked_sub(now.unix_ms)
                .filter(|value| *value > 0)
                .ok_or_else(denied)?;
            let deadline = now
                .monotonic
                .checked_add(Duration::from_millis(remaining))
                .ok_or_else(denied)?;
            Ok((not_before, expires, deadline, review_tier))
        })();
        if matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Unavailable
                | meerkat_core::OperationAuthorizationError::ObservationUnavailable(_))
        ) {
            known_failure = result.as_ref().err().copied();
        }
        result
    });
    if let Some(error) = known_failure {
        return ObservedAuthorizationResult::unobserved(Err(error));
    }
    let (result, publication) = match observed {
        Ok(observed) => observed,
        Err(error) => {
            return ObservedAuthorizationResult::unobserved(Err(publication_refusal(error)));
        }
    };
    let policy = Some(publication.policy_observation());
    let result = result.and_then(|(not_before_ms, expires_at_ms, deadline, review_tier)| {
        let prepared = CompiledLocalAuthorization {
            binding: binding.clone(),
            publication,
            clock: Arc::clone(clock),
            not_before_ms,
            expires_at_ms,
            deadline,
            review_tier,
        };
        prepared.check_current(binding)?;
        Ok(Arc::new(prepared) as Arc<dyn PreparedOperationAuthorization>)
    });
    let policy = if matches!(
        result,
        Err(meerkat_core::OperationAuthorizationError::Unavailable
            | meerkat_core::OperationAuthorizationError::ObservationUnavailable(_))
    ) {
        None
    } else {
        policy
    };
    ObservedAuthorizationResult { result, policy }
}

struct CompiledLocalAuthorization {
    binding: PreparedAuthorizationBinding,
    publication: LocalPublicationStamp,
    clock: Arc<dyn LocalAuthorizationClock>,
    not_before_ms: u64,
    expires_at_ms: u64,
    deadline: Instant,
    review_tier: OperationReviewTier,
}

impl PreparedOperationAuthorization for CompiledLocalAuthorization {
    fn policy_observation(
        &self,
    ) -> Option<meerkat_core::authorization::PolicyPublicationObservation> {
        Some(self.publication.policy_observation())
    }

    fn review_tier(&self) -> OperationReviewTier {
        self.review_tier
    }

    fn validity(&self) -> Option<meerkat_core::authorization::OperationValidity> {
        Some(meerkat_core::authorization::OperationValidity {
            not_before_ms: self.not_before_ms,
            expires_at_ms: self.expires_at_ms,
            deadline: self.deadline,
        })
    }

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        if !self.binding.same_operation(binding) {
            return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
        }
        // A local time read precedes the final publication read, so no relevant
        // awaited step or policy traversal occurs after the final current check.
        let now = self
            .clock
            .now()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        if now.unix_ms < self.not_before_ms
            || now.unix_ms >= self.expires_at_ms
            || now.monotonic >= self.deadline
        {
            return Err(denied().into());
        }
        self.publication
            .check_current()
            .map_err(publication_refusal)
    }
}

fn publication_refusal(error: PublicationError) -> meerkat_core::OperationAuthorizationError {
    match error {
        PublicationError::Changed => {
            OperationRefused::new(OperationRefusalKind::ReprepareRequired).into()
        }
        PublicationError::Unavailable => meerkat_core::OperationAuthorizationError::Unavailable,
    }
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

#[cfg(test)]
mod tests;
