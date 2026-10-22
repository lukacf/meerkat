//! Authenticated scoped original work at native admission.
//!
//! A trusted host adapter authenticates a selected original on its own
//! transport and submits it as governed input. The claims travel in the
//! existing [`InputAuthorityAssociation`], and [`NativeIngressContext`] binds
//! the final submitted payload. [`ScopedOriginalIngress`] is the installed
//! [`NativeIngressCheck`] that authenticates every one of those claims against
//! its actual owner before acceptance:
//!
//! - the ingress namespace is the installed ingress domain (authority and
//!   namespace, compared as one `ResourceDomain`);
//! - the adapter's own custody view of this exact original occurrence names
//!   the same original (its source authority and work), the same occurrence,
//!   original authentication, root event and ordered source contributors. The
//!   original's source authority is the source owner's principal; it is not
//!   required to be the ingress authority;
//! - the requester may invoke the executor and represented-subject mandate
//!   (the host mandate owner), and a grant-lineage mandate resolves in the
//!   actual grant owner for that executor and subject;
//! - every named contributor is an admitted original the adapter still
//!   retains, with an active disposition, whose own grant-lineage mandate is
//!   live in the grant owner for its own grantee and subject. Liveness is no
//!   operation authority: each operation is narrowed by the contributor's
//!   current ceiling and still needs the work's own authority;
//! - the destination is the one the adapter authenticated for this original
//!   (owner, runtime, context, generation and audience), on the admitting
//!   runtime. Whether this session document currently serves that
//!   destination is the machine-owned scoped context binding's check, in the
//!   same admission; this module keeps no second destination owner.
//!
//! First submission and retry are told apart by the runtime's native replay
//! lookup, which the driver resolves before this check and attaches to the
//! ingress context ([`NativeIngressContext::replay_lookup`]): the qualified
//! idempotency key (the raw event key qualified by requester, target and
//! namespace) only selects a candidate row, and the exact replay digest of
//! the final input (content and the complete association) decides between a
//! replay and a conflict. A first authentication must be the original's own
//! authentication observation; an exact retained replay may carry a fresh
//! transport authentication. A check without a verdict (a persistent
//! runtime's pre-admission check) applies neither rule; its decisive call
//! carries one. No caller flag and no authentication reference decides it.
//!
//! It keeps no registry of originals or accepted inputs, and it
//! never promotes a stored association, a consent event or a destination
//! binding into authority. Every later operation still resolves current
//! authority through the existing run and operation owners.

use std::sync::Arc;

use meerkat_authorization::grant_policy::{AdmittedWorkPolicyOwner, OperationPolicyOwner};
use meerkat_authorization::grants::{GrantRefusal, LineageResolutionRefusal, LocalGrantAuthority};
use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_authorization_contracts::evidence::{EvidenceId, HistoricalEvidenceRef};
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace,
    WorkAuthorityBasis,
};
use meerkat_core::authorization::ControllerReadinessFailure;
use meerkat_core::{OperationRefusalKind, OperationRefused, PrincipalRef};

use super::local_authorization::{
    NativeGrantWorkConfiguration, NativeIngressCheck, NativeRetainedResumeCheck,
};
use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::input::Input;
use crate::input_authority::{
    NativeAdmissionError, NativeIngressContext, NativeReplayLookup, NativeWorkAuthorizationHost,
};
use crate::traits::RuntimeDriverError;
use meerkat_core::mcp_attachment::ConfiguredMcpAttachmentAuthority;

/// The host adapter's custody view of one original occurrence, as its
/// transport established it. Read for one admission; never stored by the
/// runtime, and never built from the submitted claims.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthenticatedOriginal {
    /// The original as its source owner names it.
    pub original_work: OriginalWorkRef,
    /// The occurrence of that original within the ingress namespace.
    pub occurrence_scope: EvidenceId,
    /// The authentication observation of the original event itself.
    pub original_authentication: HistoricalEvidenceRef,
    pub root_event: HistoricalEvidenceRef,
    /// Every source contributor of the original, in its order.
    pub contributing_work: Vec<OriginalWorkRef>,
    /// The destination the original was addressed to.
    pub target: NativeWorkTarget,
    /// The original's own mandate as its source owner holds it.
    pub authority_basis: WorkAuthorityBasis,
}

impl std::fmt::Debug for AuthenticatedOriginal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthenticatedOriginal([REDACTED])")
    }
}

/// Trusted host adapter seam: its custody view of the original occurrence
/// that the current ingress observation authenticated, looked up by the
/// claimed namespace and original. It is also asked for each contributor the
/// original names, which must be an actually admitted original of the same
/// requester in its custody. `Ok(None)` means this ingress
/// authenticated no such original; an error means the adapter cannot say
/// now. The view is the adapter's own record, so a claim that disagrees with
/// it on any field is refused.
pub trait AuthenticatedOriginalSource: Send + Sync {
    /// Native admission calls this synchronous method while it holds the
    /// session's mutation gate and driver lock, so it performs no I/O, never
    /// blocks and takes no native lock its caller may already hold.
    fn authenticated_original(
        &self,
        ingress: &NativeIngressContext,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure>;

    /// The source owner's current custody of an original, read without a
    /// live ingress observation: for every operation of work that names the
    /// original, and for every retained resume of it. An error means the
    /// owner cannot say now.
    ///
    /// The owner answers [`RetainedCustody::AdmissionPending`] only for a
    /// candidate it recorded and will settle itself, within a bound it owns:
    /// its own recovery, trigger, budget and close, never a reader's wait. It
    /// settles every candidate it records. It promotes the candidate once it
    /// has verified that the runtime admitted that exact input, or it
    /// withdraws its custody of the candidate for good. It withdraws any
    /// candidate it cannot settle within its bound, and once it stops it
    /// answers no candidate as `AdmissionPending`. A withdrawal is final: the
    /// owner never promotes that candidate later, and the original then
    /// answers [`RetainedCustody::NotRetained`], the terminal answer. A
    /// withdrawal ends only the owner's custody. It is no fact about the
    /// runtime's input, which the owner never reports as not admitted,
    /// cancelled or settled because its bound expired. After a promotion or a
    /// withdrawal the host announces it to whatever may wait on it (for a
    /// retained resume, `ContinuationHostBindings::source_custody_changed` in
    /// the `meerkat` crate). No reader waits on `AdmissionPending`; a caller
    /// that retries is bounded by this obligation.
    ///
    /// The same publication contract as the native admitted-work owner
    /// applies: every relevant custody change, a promotion and a withdrawal
    /// included, uses the same publication as the grant authority, and this
    /// synchronous method performs no I/O and takes no native lock its caller
    /// may already hold. A retained mandate is a claim of the source owner's
    /// own custody; its grant is resolved again, currently, in the grant
    /// owner.
    fn retained_custody(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<RetainedCustody, ControllerReadinessFailure>;

    /// The original owner's current operational disposition of an original
    /// it authenticated: whether its operations are cancelled. Read for the
    /// work's own original at every operation and admission (whether or not
    /// the owner has promoted its admission) and for every named
    /// contributor. A cancelled original keeps its immutable identity and
    /// controller evidence; only its operations are refused.
    ///
    /// The same contract as [`Self::retained_custody`]: a cancellation is
    /// published on the grant authority's publication, the read performs no
    /// I/O, and an error (the owner cannot say now, or its cancellation
    /// write is uncertain) is unavailability, never permission. The default
    /// is for owners that never cancel an original.
    fn original_disposition(
        &self,
        _namespace: &QualifiedIngressNamespace,
        _original: &OriginalWorkRef,
    ) -> Result<OriginalDisposition, ControllerReadinessFailure> {
        Ok(OriginalDisposition::Active)
    }

    /// The source owner's current enrollment of a SERVICE original: a
    /// standing original the trusted host enrolled, under an operator's
    /// grant, for work that no runtime input carries (a scheduled host
    /// runnable's provider calls, admitted with
    /// [`crate::MeerkatMachine::prepare_service_call`]). The answer is the
    /// exact association the owner enrolled; the native host compares it
    /// with the claims of every call.
    ///
    /// A service original is never admitted and never retained:
    /// [`Self::retained_custody`] answers [`RetainedCustody::NotRetained`]
    /// for it, always, so it can never stand for admitted work (an
    /// operation's contributor, a retained resume); and an admitted
    /// original is never answered here, so it can never back a service
    /// call. `Ok(None)`: no current enrollment, because the original was
    /// never enrolled or was revoked (final). Cancellation stays
    /// [`Self::original_disposition`]'s. An error means the owner cannot say
    /// now.
    ///
    /// The same contract as [`Self::retained_custody`]: every enrollment and
    /// revocation is published on the grant authority's publication, and
    /// this synchronous method performs no I/O and takes no native lock its
    /// caller may already hold. The default enrolls nothing.
    fn enrolled_service_original(
        &self,
        _namespace: &QualifiedIngressNamespace,
        _original: &OriginalWorkRef,
    ) -> Result<Option<EnrolledServiceOriginal>, ControllerReadinessFailure> {
        Ok(None)
    }
}

/// The source owner's record of one enrolled service original
/// ([`AuthenticatedOriginalSource::enrolled_service_original`]): the exact
/// association it enrolled. Data the native host compares claims with,
/// never a permission: every call is still decided by the work owner, the
/// operator's grant and the operation owner.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EnrolledServiceOriginal {
    association: InputAuthorityAssociation,
}

impl EnrolledServiceOriginal {
    /// The record of `association`, as its owner enrolled it.
    #[must_use]
    pub fn new(association: InputAuthorityAssociation) -> Self {
        Self { association }
    }

    /// The enrolled association.
    #[must_use]
    pub fn association(&self) -> &InputAuthorityAssociation {
        &self.association
    }
}

impl std::fmt::Debug for EnrolledServiceOriginal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EnrolledServiceOriginal([REDACTED])")
    }
}

/// The source owner's current custody of one original, for every reader
/// that needs it as admitted custody
/// ([`AuthenticatedOriginalSource::retained_custody`]).
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetainedCustody {
    /// Admitted and held: the original's own mandate.
    Retained(WorkAuthorityBasis),
    /// The owner's submission candidate, whose admission it has not
    /// settled yet and will settle itself, within its own bound. A not-yet
    /// answer: never a verdict, and no fact about the runtime's input.
    AdmissionPending,
    /// Not held: never admitted here, released, withdrawn, or after a
    /// restart. For a withdrawn candidate this is final.
    NotRetained,
}

impl std::fmt::Debug for RetainedCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Retained(_) => "Retained([REDACTED])",
            Self::AdmissionPending => "AdmissionPending",
            Self::NotRetained => "NotRetained",
        })
    }
}

/// One original's custody as every native reader classifies it. A definite
/// cancellation beats a pending admission: the disposition is read before a
/// pending admission is answered, so a cancelled original stays a verdict.
/// Readers that already read the disposition first keep that read.
pub(crate) enum CustodyClass {
    Retained(WorkAuthorityBasis),
    /// Not yet: the owner will settle its candidate within its own bound.
    Pending,
    /// A pending candidate whose operations are cancelled.
    Cancelled,
    NotRetained,
}

/// The one classifier every reader of retained custody uses.
pub(crate) fn classify_custody(
    originals: &dyn AuthenticatedOriginalSource,
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
) -> Result<CustodyClass, ControllerReadinessFailure> {
    Ok(match originals.retained_custody(namespace, original)? {
        RetainedCustody::Retained(basis) => CustodyClass::Retained(basis),
        RetainedCustody::AdmissionPending => {
            match originals.original_disposition(namespace, original)? {
                OriginalDisposition::Active => CustodyClass::Pending,
                OriginalDisposition::OperationCancelled { .. } => CustodyClass::Cancelled,
            }
        }
        RetainedCustody::NotRetained => CustodyClass::NotRetained,
    })
}

/// The original owner's current operational disposition of one original
/// ([`AuthenticatedOriginalSource::original_disposition`]).
#[derive(Clone, PartialEq, Eq)]
pub enum OriginalDisposition {
    /// Not cancelled: current, whether or not its admission is promoted.
    Active,
    /// Its operations are cancelled. `evidence` is the owner's own record of
    /// the cancellation.
    OperationCancelled { evidence: HistoricalEvidenceRef },
}

impl std::fmt::Debug for OriginalDisposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Active => "Active",
            Self::OperationCancelled { .. } => "OperationCancelled",
        })
    }
}

/// Refuse operations of a cancelled original: a definite cancellation is a
/// local denial, an unobservable disposition is unavailability.
/// The current grant one named original contributes to the operation of
/// `binding`: the original must be active and still retained by its source
/// owner, and its retained basis selects the lineage for this operation's
/// target, resolved for `executor`/`represented_subject` in the grant owner.
/// `None` is a host-policy or service-mandate basis, decided by its own
/// owner. Each original resolves its own lineage; every one must allow the
/// operation.
///
/// # Errors
/// A cancelled, no longer retained or revoked original, or a targeted basis
/// with no lineage for this target, is denied; an owner that cannot say now
/// is unavailable.
pub fn original_operation_grant(
    grants: &LocalGrantAuthority,
    originals: &dyn AuthenticatedOriginalSource,
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
    binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    executor: &PrincipalRef,
    represented_subject: Option<&PrincipalRef>,
) -> Result<
    Option<meerkat_authorization::grants::ResolvedGrant>,
    meerkat_core::OperationAuthorizationError,
> {
    let denied = || {
        meerkat_core::OperationAuthorizationError::from(OperationRefused::new(
            OperationRefusalKind::Denied,
        ))
    };
    require_active_operation(originals, namespace, original)?;
    let basis = operation_retained_basis(originals, namespace, original)?.ok_or_else(denied)?;
    let lineage = match &basis {
        WorkAuthorityBasis::HostPolicy { .. } | WorkAuthorityBasis::ServiceMandate { .. } => {
            return Ok(None);
        }
        WorkAuthorityBasis::GrantLineage { lineage } => lineage.as_slice(),
        WorkAuthorityBasis::TargetedGrantLineages { .. } => {
            let target = meerkat_authorization::grant_policy::operation_grant_target(binding)?;
            basis.lineage_for(target.as_ref()).ok_or_else(denied)?
        }
    };
    grants
        .resolve_lineage(lineage, executor, represented_subject)
        .map(Some)
        .map_err(|refusal| match refusal {
            GrantRefusal::Denied => denied(),
            GrantRefusal::ControllerInUse | GrantRefusal::Duplicate | GrantRefusal::Unavailable => {
                meerkat_core::OperationAuthorizationError::Unavailable
            }
        })
}

/// The ceiling one contributor imposes on an operation of the work that
/// names it ([`NamedContributorWork`]). The original must be active and
/// still retained by its source owner, and its retained basis selects the
/// lineage for this operation's target exactly as for
/// [`original_operation_grant`]. That lineage is resolved for its own leaf
/// grantee and represented subject
/// ([`LocalGrantAuthority::resolve_lineage_ceiling`]), because a
/// contributor may be another executor's original. The ceiling only
/// narrows the operation: the work's own authority is resolved separately,
/// for its own executor. `None` is a host-policy or service-mandate basis,
/// decided by its own owner.
///
/// # Errors
/// A cancelled, no longer retained or no longer current original, or a
/// targeted basis with no lineage for this target, is denied; an owner that
/// cannot say now is unavailable. An owner change published while the
/// lineage is read inside the caller's own observation of the grant
/// publication is a stale refusal (`ReprepareRequired`).
fn contributor_operation_ceiling(
    grants: &LocalGrantAuthority,
    originals: &dyn AuthenticatedOriginalSource,
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
    binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
) -> Result<
    Option<meerkat_authorization::grants::ContributorCeiling>,
    meerkat_core::OperationAuthorizationError,
> {
    let denied = || {
        meerkat_core::OperationAuthorizationError::from(OperationRefused::new(
            OperationRefusalKind::Denied,
        ))
    };
    require_active_operation(originals, namespace, original)?;
    let basis = operation_retained_basis(originals, namespace, original)?.ok_or_else(denied)?;
    let lineage = match &basis {
        WorkAuthorityBasis::HostPolicy { .. } | WorkAuthorityBasis::ServiceMandate { .. } => {
            return Ok(None);
        }
        WorkAuthorityBasis::GrantLineage { lineage } => lineage.as_slice(),
        WorkAuthorityBasis::TargetedGrantLineages { .. } => {
            let target = meerkat_authorization::grant_policy::operation_grant_target(binding)?;
            basis.lineage_for(target.as_ref()).ok_or_else(denied)?
        }
    };
    grants
        .resolve_lineage_ceiling_observed(lineage)
        .map(Some)
        .map_err(|refusal| match refusal {
            // Read inside the caller's own observation of the grant
            // publication, which is stale too: a stale read it classifies
            // and may rebuild once, never an unavailable owner.
            LineageResolutionRefusal::Changed => {
                OperationRefused::new(OperationRefusalKind::ReprepareRequired).into()
            }
            LineageResolutionRefusal::Refused(GrantRefusal::Denied) => denied(),
            LineageResolutionRefusal::Refused(
                GrantRefusal::ControllerInUse | GrantRefusal::Duplicate | GrantRefusal::Unavailable,
            ) => meerkat_core::OperationAuthorizationError::Unavailable,
        })
}

/// The source owner as the authorization crate's further-original basis
/// source: an original's retained mandate when it is active and still
/// retained. It is never handed an operation.
pub struct SourceOriginalBasis(Arc<dyn AuthenticatedOriginalSource>);

impl SourceOriginalBasis {
    pub fn new(originals: Arc<dyn AuthenticatedOriginalSource>) -> Self {
        Self(originals)
    }
}

impl meerkat_authorization::work::OriginalBasisSource for SourceOriginalBasis {
    fn require_active(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        require_active_operation(self.0.as_ref(), namespace, original)
    }

    fn active_retained_basis(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<WorkAuthorityBasis>, meerkat_core::OperationAuthorizationError> {
        require_active_operation(self.0.as_ref(), namespace, original)?;
        operation_retained_basis(self.0.as_ref(), namespace, original)
    }
}

/// An operation's view of `original`'s retained custody: its mandate while
/// it is held, `None` when it is not held (or its pending candidate is
/// cancelled), and unavailable while its admission is pending or the owner
/// cannot say. A pending admission is never a refusal and never waited on.
fn operation_retained_basis(
    originals: &dyn AuthenticatedOriginalSource,
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
) -> Result<Option<WorkAuthorityBasis>, meerkat_core::OperationAuthorizationError> {
    match classify_custody(originals, namespace, original)
        .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?
    {
        CustodyClass::Retained(basis) => Ok(Some(basis)),
        CustodyClass::Pending => Err(meerkat_core::OperationAuthorizationError::Unavailable),
        CustodyClass::Cancelled | CustodyClass::NotRetained => Ok(None),
    }
}

/// The source owner has not cancelled `original`'s operations: the check a
/// work's own original needs at every operation (its mandate is the
/// association's own; no retained custody is required of it).
///
/// # Errors
/// A cancelled original is denied; an owner that cannot say now is
/// unavailable.
pub fn require_active_original(
    originals: &dyn AuthenticatedOriginalSource,
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
) -> Result<(), meerkat_core::OperationAuthorizationError> {
    require_active_operation(originals, namespace, original)
}

fn require_active_operation(
    originals: &dyn AuthenticatedOriginalSource,
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
) -> Result<(), meerkat_core::OperationAuthorizationError> {
    match originals.original_disposition(namespace, original) {
        Ok(OriginalDisposition::Active) => Ok(()),
        Ok(OriginalDisposition::OperationCancelled { .. }) => {
            Err(OperationRefused::new(OperationRefusalKind::Denied).into())
        }
        Err(_) => Err(meerkat_core::OperationAuthorizationError::Unavailable),
    }
}

/// The admission-time face of the host's invocation policy: whether the
/// authenticated requester may now invoke the association's executor for its
/// represented subject under its mandate. It is implemented by the same
/// concrete policy object that serves as the native admitted-work owner
/// ([`AdmittedWorkPolicyOwner`]) and must decide under that owner's grant
/// publication, synchronously and without I/O. It is no separate mandate
/// authority and needs no prepared operation: it is the policy's own
/// admission answer. Refusals are actual verdicts; an unavailable policy is
/// readiness.
pub trait ScopedMandateOwner: Send + Sync {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError>;
}

/// Both handles of one concrete host invocation policy, taken from the same
/// `Arc`: `admission` for the scoped original check before acceptance and
/// `admitted_work` for `NativeGrantWorkConfiguration::invocation_owner`
/// afterwards. Built only by [`ScopedInvocation::from_policy`], so the two
/// checks cannot come from different policies.
pub struct ScopedInvocation {
    admission: Arc<dyn ScopedMandateOwner>,
    admitted_work: Arc<dyn AdmittedWorkPolicyOwner>,
}

impl ScopedInvocation {
    pub fn from_policy<P>(policy: Arc<P>) -> Self
    where
        P: ScopedMandateOwner + AdmittedWorkPolicyOwner + 'static,
    {
        Self {
            admission: Arc::clone(&policy) as Arc<dyn ScopedMandateOwner>,
            admitted_work: policy,
        }
    }

    /// The same policy as the native admitted-work owner.
    pub fn admitted_work(&self) -> Arc<dyn AdmittedWorkPolicyOwner> {
        Arc::clone(&self.admitted_work)
    }
}

/// The native grant composition for scoped originals: the one construction
/// path that installs the scoped original check as the admission ingress, the
/// admitted-work face of the same host invocation policy as the native
/// invocation owner, and the requirement that every governed input name the
/// session document's current bound destination.
pub struct ScopedGrantComposition {
    pub grants: Arc<LocalGrantAuthority>,
    pub ingress_domain: ResourceDomain,
    pub originals: Arc<dyn AuthenticatedOriginalSource>,
    pub invocation: ScopedInvocation,
    pub operation_owner: Arc<dyn OperationPolicyOwner>,
    pub retained_resume: Option<Arc<NativeRetainedResumeCheck>>,
}

impl ScopedGrantComposition {
    pub fn into_configuration(self) -> NativeGrantWorkConfiguration {
        let ingress = ScopedOriginalIngress::new(
            Arc::clone(&self.grants),
            self.ingress_domain,
            Arc::clone(&self.originals),
            &self.invocation,
        )
        .into_check();
        let invocation_owner = Arc::new(NamedContributorWork {
            policy: self.invocation.admitted_work(),
            grants: Arc::clone(&self.grants),
            originals: Arc::clone(&self.originals),
        });
        NativeGrantWorkConfiguration {
            grants: self.grants,
            ingress,
            invocation_owner,
            operation_owner: self.operation_owner,
            retained_resume: self.retained_resume,
            require_scoped_destination: true,
            configured_mcp_attachment: None,
        }
    }
}

/// The admitted-work face installed for scoped originals: the host
/// invocation policy's answer, conjoined, for every operation, with the
/// current ceiling of every contributor the work names. A contributor's
/// grant-lineage mandate is resolved in the actual grant owner for its own
/// leaf grantee and represented subject (a contributor may be another
/// executor's original): its restrictions narrow the operation, its
/// deadline bounds it, and a revoked, expired, cancelled or no longer
/// retained contributor refuses the operation locally. A ceiling never
/// supplies authority: the work's own mandate is resolved for its executor
/// by the grant-backed policy, and the operation owner still decides the
/// operation. Controller entry is resolved separately and is not narrowed by
/// contributors. The review mandate is the host invocation policy's, read
/// under the same original and contributor checks as an operation, and
/// followed by the narrowing of the contributors' current ceilings.
struct NamedContributorWork {
    policy: Arc<dyn AdmittedWorkPolicyOwner>,
    grants: Arc<LocalGrantAuthority>,
    originals: Arc<dyn AuthenticatedOriginalSource>,
}

impl NamedContributorWork {
    /// The ceiling, for the operation of `binding`, of every contributor
    /// the work names, in order, once the work's own original is current.
    /// Every contributor must be current; a host-policy or service-mandate
    /// contributor adds no ceiling.
    fn current_contributor_ceilings(
        &self,
        association: &InputAuthorityAssociation,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<
        Vec<meerkat_authorization::grants::ContributorCeiling>,
        meerkat_core::OperationAuthorizationError,
    > {
        let claims = association.candidate();
        // The work's own original: current unless its owner cancelled its
        // operations; no admission promotion is required of it.
        require_active_operation(
            self.originals.as_ref(),
            &claims.ingress_namespace,
            &claims.original_work,
        )?;
        let mut ceilings = Vec::with_capacity(claims.contributing_work.len());
        for contributor in &claims.contributing_work {
            ceilings.extend(contributor_operation_ceiling(
                &self.grants,
                self.originals.as_ref(),
                &claims.ingress_namespace,
                contributor,
                binding,
            )?);
        }
        Ok(ceilings)
    }
}

impl AdmittedWorkPolicyOwner for NamedContributorWork {
    fn authorize_context_control(
        &self,
        control: &meerkat_core::service::SystemContextControlRequest,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::WorkOwnerAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        self.policy
            .authorize_context_control(control, binding, now_ms)
    }

    /// The record of an enrolled service original, from the source this
    /// composition holds, while its operations are not cancelled. A
    /// cancelled original is `Denied`; a source that cannot say now is
    /// `Unavailable`. Only the record: the native host compares it with the
    /// presented claims, and every operation is still decided by the host
    /// invocation policy, the grant and the operation owner.
    fn enrolled_service_original(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<InputAuthorityAssociation>, meerkat_core::OperationAuthorizationError> {
        require_active_operation(self.originals.as_ref(), namespace, original)?;
        self.originals
            .enrolled_service_original(namespace, original)
            .map(|record| record.map(|record| record.association))
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)
    }

    // An attachment control names no contributor work; the host invocation
    // policy decides it, as for context control.
    fn authorize_mcp_attachment(
        &self,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::WorkOwnerAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        self.policy
            .authorize_mcp_attachment(control, binding, now_ms)
    }

    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        purpose: meerkat_authorization::policy::LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::WorkOwnerAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        let mut allowance =
            self.policy
                .authorize_admitted_work(association, binding, purpose, now_ms)?;
        if purpose != meerkat_authorization::policy::LocalPolicyPurpose::Operation {
            return Ok(allowance);
        }
        for ceiling in self.current_contributor_ceilings(association, binding)? {
            allowance.restrictions = allowance.restrictions.conjoin(ceiling.restrictions());
            allowance.expires_at_ms = allowance.expires_at_ms.min(ceiling.expires_at_ms());
        }
        Ok(allowance)
    }

    /// The host invocation policy's current mandate, forwarded only while
    /// the work's own original and every contributor it names are current,
    /// as for its operations. The contributors' current ceilings narrow
    /// every operation of the work, so their conjunction and earliest
    /// expiry follow the mandate ([`review_bounds_text`]); a contributor
    /// adds no mandate of its own, and a work naming none is unchanged.
    fn review_mandate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        let mandate = self.policy.review_mandate(association, binding, now_ms)?;
        let ceilings = self.current_contributor_ceilings(association, binding)?;
        if ceilings.is_empty() {
            return Ok(mandate);
        }
        let mut restrictions =
            meerkat_authorization_contracts::constraints::ExecutionRestrictions::unrestricted();
        let mut expires_at_ms = u64::MAX;
        for ceiling in &ceilings {
            restrictions = restrictions.conjoin(ceiling.restrictions());
            expires_at_ms = expires_at_ms.min(ceiling.expires_at_ms());
        }
        let narrowing = review_bounds_text(&restrictions, expires_at_ms, now_ms)?;
        Ok(Some(match mandate {
            Some(mandate) => format!("{mandate} Contributing originals narrow it to: {narrowing}"),
            None => format!("Contributing originals narrow this work to: {narrowing}"),
        }))
    }
}

/// Bounded passive text of execution bounds for review and consent
/// presentation: the exact values of the action, resource, processor and
/// audience bounds, each value in its own serialized form (a principal with
/// its kind, id and qualification), "any" where a dimension is unbounded,
/// "owned by" and the owners where the resource bound names the authorities
/// that own its domains, and the effective expiry, the earlier of
/// `expires_at_ms` and the end of the bounds' own lifetime window (or "no
/// expiry"). It is data only and grants nothing.
///
/// # Errors
/// Bounds that permit nothing at `now_ms` (an empty set, an empty, not yet
/// open or ended lifetime, or `expires_at_ms` passed) or that carry an
/// unresolved fact are refused as at authorization, where they admit no
/// operation: [`OperationRefusalKind::Denied`]. A value that cannot be
/// serialized is unavailable.
pub fn review_bounds_text(
    restrictions: &meerkat_authorization_contracts::constraints::ExecutionRestrictions,
    expires_at_ms: u64,
    now_ms: u64,
) -> Result<String, meerkat_core::OperationAuthorizationError> {
    use meerkat_authorization_contracts::constraints::{
        DomainBound, DomainRestriction, ExactRestriction, LifetimeBound, SetBound,
    };
    fn permits_none<T: Ord>(restriction: &ExactRestriction<T>) -> bool {
        !restriction.unresolved_facts().is_empty()
            || matches!(restriction.bound(), SetBound::Exact(values) if values.is_empty())
    }
    fn listed<T: Ord + serde::Serialize>(
        restriction: &ExactRestriction<T>,
    ) -> Result<String, meerkat_core::OperationAuthorizationError> {
        match restriction.bound() {
            SetBound::Unrestricted => Ok("any".to_owned()),
            SetBound::Exact(values) => serde_json::to_string(values)
                .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable),
        }
    }
    // An owner bound is never empty, so only an empty exact set or an
    // unresolved fact permits no domain.
    fn domains_permit_none(restriction: &DomainRestriction) -> bool {
        !restriction.unresolved_facts().is_empty()
            || matches!(restriction.bound(), DomainBound::Exact(values) if values.is_empty())
    }
    // A bound kind this text cannot name is not rendered as "any": it is
    // unavailable, as an unserializable value is.
    fn listed_domains(
        restriction: &DomainRestriction,
    ) -> Result<String, meerkat_core::OperationAuthorizationError> {
        let unavailable = |_| meerkat_core::OperationAuthorizationError::Unavailable;
        match restriction.bound() {
            DomainBound::Unrestricted => Ok("any".to_owned()),
            DomainBound::Exact(values) => serde_json::to_string(values).map_err(unavailable),
            DomainBound::OwnedBy(owners) => serde_json::to_string(owners)
                .map(|owners| format!("owned by {owners}"))
                .map_err(unavailable),
            _ => Err(meerkat_core::OperationAuthorizationError::Unavailable),
        }
    }
    let denied = || {
        meerkat_core::OperationAuthorizationError::from(OperationRefused::new(
            OperationRefusalKind::Denied,
        ))
    };
    if permits_none(&restrictions.actions)
        || domains_permit_none(&restrictions.resource_domains)
        || permits_none(&restrictions.processors)
        || permits_none(&restrictions.audiences)
        || !restrictions.lifetime.unresolved_facts().is_empty()
        || !restrictions.delegation_depth.unresolved_facts().is_empty()
    {
        return Err(denied());
    }
    let until = match restrictions.lifetime.bound() {
        LifetimeBound::Unrestricted => expires_at_ms,
        LifetimeBound::Window {
            not_before_ms,
            expires_at_ms: window_end,
        } if not_before_ms <= now_ms => expires_at_ms.min(window_end),
        LifetimeBound::Window { .. } | LifetimeBound::Empty => return Err(denied()),
    };
    if until <= now_ms {
        return Err(denied());
    }
    let until = if until == u64::MAX {
        "no expiry".to_owned()
    } else {
        i64::try_from(until)
            .ok()
            .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
            .map_or_else(
                || format!("unix ms {until}"),
                |at| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            )
    };
    let actions = listed(&restrictions.actions)?;
    let resources = listed_domains(&restrictions.resource_domains)?;
    let processors = listed(&restrictions.processors)?;
    let audiences = listed(&restrictions.audiences)?;
    Ok(format!(
        "actions {actions}; resources {resources}; processors {processors}; audiences \
         {audiences}; effective until {until}."
    ))
}

/// The installed host of a governed realm whose trusted owners are not
/// available (its grant owner did not restore, or the embedding supplied no
/// policy, originals or operation owners). Governance stays selected: every
/// governed admission is refused as readiness `PolicyUnavailable`, every
/// configured MCP server is refused as unavailable before discovery or spawn
/// (the trait's default), and governed work requires a bound destination.
/// It issues nothing, creates no owner or store and never selects ungoverned
/// behavior.
struct UnavailableScopedHost;

impl crate::input_authority::NativeWorkAuthorizationHost for UnavailableScopedHost {
    fn authenticate_association(
        &self,
        _runtime_id: &LogicalRuntimeId,
        _input: &Input,
        _ingress: &NativeIngressContext,
        _association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        Err(NativeAdmissionError::Readiness(
            ControllerReadinessFailure::PolicyUnavailable,
        ))
    }

    fn work_context(
        &self,
        _batch: &crate::input_authority::NativeWorkBatch,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        crate::input_authority::NativeWorkContextError,
    > {
        Err(crate::input_authority::NativeWorkContextError::UnsupportedController)
    }

    fn requires_scoped_destination(&self) -> bool {
        true
    }
}

impl super::MeerkatMachine {
    /// A persistent machine for a realm whose governed posture the trusted
    /// host selected but whose owners are unavailable now. It is the same
    /// governed profile and execution custody as
    /// [`super::MeerkatMachine::persistent_with_local_grant_authorization`],
    /// with an installed host that refuses locally and typed (see
    /// `UnavailableScopedHost`). Restoring the owners needs a new machine.
    pub fn persistent_with_unavailable_scoped_governance(
        store: Arc<dyn crate::store::RuntimeStore>,
        blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
    ) -> Result<Self, RuntimeDriverError> {
        Self::persistent_with_mode(store, blob_store, true)?
            .install_native_work_authorization(|_| Arc::new(UnavailableScopedHost))
    }

    /// [`super::MeerkatMachine::persistent_with_local_grant_authorization`]
    /// whose host decides each configured MCP server of a session through
    /// `configured_mcp`, a trusted owner of the host composition (for
    /// example the realm's own persisted installations). Every other
    /// decision is the native/grant host's. Without this, a governed host
    /// refuses every configured server as unavailable.
    pub fn persistent_with_local_grant_authorization_and_configured_mcp(
        store: Arc<dyn crate::store::RuntimeStore>,
        blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
        configuration: NativeGrantWorkConfiguration,
        configured_mcp: Arc<dyn ConfiguredMcpAttachmentAuthority>,
    ) -> Result<Self, RuntimeDriverError> {
        Self::persistent_with_mode(store, blob_store, true)?.install_native_work_authorization(
            |machine| {
                Arc::new(WithConfiguredMcpAttachment {
                    host: super::local_authorization::local_grant_host(configuration, machine),
                    configured_mcp,
                    per_agent: None,
                })
            },
        )
    }

    /// [`Self::persistent_with_local_grant_authorization_and_configured_mcp`]
    /// whose configured-MCP decision is also per agent: each session's
    /// bound agent must currently hold a grant naming the installation
    /// ([`super::AgentInstallationGrant`]), and `policy` (the composition's
    /// operation policy for the attachment) must allow it. `None` refuses
    /// every configured server as unavailable.
    pub fn persistent_with_local_grant_authorization_and_agent_mcp(
        store: Arc<dyn crate::store::RuntimeStore>,
        blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
        configuration: NativeGrantWorkConfiguration,
        configured_mcp: Arc<dyn ConfiguredMcpAttachmentAuthority>,
        agent_grant: super::AgentInstallationGrant,
        policy: Option<Arc<dyn ConfiguredMcpAttachmentAuthority>>,
    ) -> Result<Self, RuntimeDriverError> {
        Self::persistent_with_mode(store, blob_store, true)?.install_native_work_authorization(
            |machine| {
                Arc::new(WithConfiguredMcpAttachment {
                    host: super::local_authorization::local_grant_host(configuration, machine),
                    configured_mcp,
                    per_agent: Some((agent_grant, policy)),
                })
            },
        )
    }
}

impl super::StagedMeerkatMachine {
    /// Install the configured-MCP native/grant host on the staged governed
    /// machine, exactly as
    /// [`super::MeerkatMachine::persistent_with_local_grant_authorization_and_configured_mcp`]
    /// does on a freshly opened one.
    pub fn install_local_grant_authorization_and_configured_mcp(
        self,
        configuration: NativeGrantWorkConfiguration,
        configured_mcp: Arc<dyn ConfiguredMcpAttachmentAuthority>,
    ) -> Result<super::MeerkatMachine, RuntimeDriverError> {
        self.machine.install_native_work_authorization(|machine| {
            Arc::new(WithConfiguredMcpAttachment {
                host: super::local_authorization::local_grant_host(configuration, machine),
                configured_mcp,
                per_agent: None,
            })
        })
    }

    /// Install the per-agent configured-MCP native/grant host on the staged
    /// governed machine, exactly as
    /// [`super::MeerkatMachine::persistent_with_local_grant_authorization_and_agent_mcp`]
    /// does on a freshly opened one. Refuses while a clone of the stage is
    /// shared or a session is registered.
    pub fn install_local_grant_authorization_and_agent_mcp(
        self,
        configuration: NativeGrantWorkConfiguration,
        configured_mcp: Arc<dyn ConfiguredMcpAttachmentAuthority>,
        agent_grant: super::AgentInstallationGrant,
        policy: Option<Arc<dyn ConfiguredMcpAttachmentAuthority>>,
    ) -> Result<super::MeerkatMachine, RuntimeDriverError> {
        self.machine.install_native_work_authorization(|machine| {
            Arc::new(WithConfiguredMcpAttachment {
                host: super::local_authorization::local_grant_host(configuration, machine),
                configured_mcp,
                per_agent: Some((agent_grant, policy)),
            })
        })
    }

    /// Install the governed host whose owners are unavailable on the staged
    /// machine, as
    /// [`super::MeerkatMachine::persistent_with_unavailable_scoped_governance`]
    /// does: every governed admission is refused as unavailable.
    pub fn install_unavailable_scoped_governance(
        self,
    ) -> Result<super::MeerkatMachine, RuntimeDriverError> {
        self.machine
            .install_native_work_authorization(|_| Arc::new(UnavailableScopedHost))
    }
}

#[cfg(test)]
impl super::MeerkatMachine {
    /// A storeless machine with the per-agent configured-MCP host of
    /// [`Self::persistent_with_local_grant_authorization_and_agent_mcp`].
    pub(crate) fn with_local_grant_authorization_and_agent_mcp(
        self,
        configuration: NativeGrantWorkConfiguration,
        configured_mcp: Arc<dyn ConfiguredMcpAttachmentAuthority>,
        agent_grant: super::AgentInstallationGrant,
        policy: Option<Arc<dyn ConfiguredMcpAttachmentAuthority>>,
    ) -> Result<Self, RuntimeDriverError> {
        self.install_native_work_authorization(|machine| {
            Arc::new(WithConfiguredMcpAttachment {
                host: super::local_authorization::local_grant_host(configuration, machine),
                configured_mcp,
                per_agent: Some((agent_grant, policy)),
            })
        })
    }
}

/// A governed host whose configured-MCP authority is the trusted
/// composition's own. It delegates every other method, including the
/// defaulted ones, to the wrapped host: a new trait method must be
/// delegated here too, or this host would answer it with the default.
struct WithConfiguredMcpAttachment {
    host: Arc<dyn NativeWorkAuthorizationHost>,
    configured_mcp: Arc<dyn ConfiguredMcpAttachmentAuthority>,
    /// The per-agent conjunct and the composition's attachment policy.
    per_agent: Option<(
        super::AgentInstallationGrant,
        Option<Arc<dyn ConfiguredMcpAttachmentAuthority>>,
    )>,
}

impl NativeWorkAuthorizationHost for WithConfiguredMcpAttachment {
    fn configured_mcp_attachment(
        &self,
        _session_id: &meerkat_core::SessionId,
    ) -> Arc<dyn ConfiguredMcpAttachmentAuthority> {
        Arc::clone(&self.configured_mcp)
    }

    fn configured_mcp_attachment_for_agent(
        &self,
        registration: &super::RuntimeSessionRegistrationWitness,
        agent: super::SessionAgentBinding,
    ) -> Arc<dyn ConfiguredMcpAttachmentAuthority> {
        match &self.per_agent {
            Some((grant, policy)) => grant.for_agent(
                super::ConfiguredMcpConjuncts {
                    installation: Arc::clone(&self.configured_mcp),
                    policy: policy.clone(),
                },
                agent,
            ),
            None => self.configured_mcp_attachment(registration.session_id()),
        }
    }

    fn mcp_attachment_agent_selection(
        &self,
        agent: super::SessionAgentBinding,
    ) -> Option<Arc<dyn ConfiguredMcpAttachmentAuthority>> {
        match &self.per_agent {
            Some((grant, _)) => Some(grant.agent_selection(agent)),
            None => self.host.mcp_attachment_agent_selection(agent),
        }
    }

    fn context_control_authorization(
        &self,
        control: Arc<meerkat_core::service::SystemContextControlRequest>,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_core::OperationAuthorizationError,
    > {
        self.host.context_control_authorization(control)
    }

    fn mcp_attachment_authorization(
        &self,
        control: Arc<meerkat_core::mcp_attachment_control::McpAttachmentControlRequest>,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_core::OperationAuthorizationError,
    > {
        self.host.mcp_attachment_authorization(control)
    }

    fn service_call_authorization(
        &self,
        request: Arc<crate::input_authority::ServiceCallRequest>,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_core::OperationAuthorizationError,
    > {
        self.host.service_call_authorization(request)
    }

    fn authenticate_association(
        &self,
        runtime_id: &LogicalRuntimeId,
        input: &Input,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        self.host
            .authenticate_association(runtime_id, input, ingress, association)
    }

    fn work_context(
        &self,
        batch: &crate::input_authority::NativeWorkBatch,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        crate::input_authority::NativeWorkContextError,
    > {
        self.host.work_context(batch)
    }

    fn requires_scoped_destination(&self) -> bool {
        self.host.requires_scoped_destination()
    }

    fn authenticate_retained_resume(
        &self,
        runtime_id: &LogicalRuntimeId,
        input: &Input,
        evidence: &crate::retained_work::RetainedResumeEvidence,
    ) -> Result<meerkat_core::ControllerModelClient, crate::retained_work::RetainedResumeError>
    {
        self.host
            .authenticate_retained_resume(runtime_id, input, evidence)
    }
}

/// The installed authenticated-original check. Every owner is a trusted
/// owner chosen by the embedding composition, never by claims.
pub struct ScopedOriginalIngress {
    grants: Arc<LocalGrantAuthority>,
    ingress_domain: ResourceDomain,
    originals: Arc<dyn AuthenticatedOriginalSource>,
    mandates: Arc<dyn ScopedMandateOwner>,
}

impl ScopedOriginalIngress {
    /// The check over the installed grant owner, ingress domain and adapter,
    /// with the admission face of the same invocation policy whose
    /// admitted-work face the composition installs as `invocation_owner`.
    pub fn new(
        grants: Arc<LocalGrantAuthority>,
        ingress_domain: ResourceDomain,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        invocation: &ScopedInvocation,
    ) -> Self {
        Self {
            grants,
            ingress_domain,
            originals,
            mandates: Arc::clone(&invocation.admission),
        }
    }

    /// The check as the existing ingress callback of
    /// `NativeGrantWorkConfiguration::ingress`.
    pub fn into_check(self) -> Arc<NativeIngressCheck> {
        let this = Arc::new(self);
        Arc::new(move |runtime, input, ingress, association| {
            this.check(runtime, input, ingress, association)
        })
    }

    /// Authenticate every claim of `association` for this admission. The
    /// existing host has already required the association's requester,
    /// ingress actor and realm to equal the process ingress, and validates
    /// the controller after this check.
    pub fn check(
        &self,
        runtime: &LogicalRuntimeId,
        _input: &Input,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        let claims = association.candidate();
        if claims.ingress_namespace.ingress != self.ingress_domain {
            return Err(refused(OperationRefusalKind::MalformedFacts));
        }
        let original = self
            .originals
            .authenticated_original(ingress, &claims.ingress_namespace, &claims.original_work)
            .map_err(NativeAdmissionError::Readiness)?
            .ok_or_else(|| refused(OperationRefusalKind::MalformedFacts))?;
        if original.original_work != claims.original_work
            || original.occurrence_scope != claims.ingress_namespace.occurrence_scope
            || original.original_authentication != claims.original_authentication
            || original.root_event != claims.root_event
            || original.contributing_work != claims.contributing_work
            || original.target != claims.target
            || original.authority_basis != claims.authority_basis
            || claims.target.logical_runtime.as_str() != runtime.to_string()
        {
            return Err(refused(OperationRefusalKind::MalformedFacts));
        }
        if ingress.replay_lookup() == Some(&NativeReplayLookup::FirstAuthentication)
            && ingress.authentication() != &claims.original_authentication
        {
            return Err(refused(OperationRefusalKind::MalformedFacts));
        }
        // A cancelled original is not admitted again, by a first submission
        // or by a replay of it.
        self.require_active_admission(&claims.ingress_namespace, &claims.original_work)?;
        self.mandates.authorize_invocation(ingress, association)?;
        self.resolve_basis(&original.authority_basis, association)?;
        // Every named contributor is itself an actually admitted original of
        // this requester in the adapter's custody, and the liveness of its
        // own mandate is a further conjunct: a contributor whose grant is no
        // longer current refuses the original that names it. The
        // requester-bound lookup alone cannot tell a contributor from a
        // submission's own pending original, so the adapter's retained
        // (admitted) custody of the contributor is required as well, and is
        // read first: a contributor whose admission its owner has not settled
        // yet is not yet admissible, never a malformed claim. A cancelled one
        // is still refused.
        for contributor in &claims.contributing_work {
            match classify_custody(
                self.originals.as_ref(),
                &claims.ingress_namespace,
                contributor,
            )
            .map_err(NativeAdmissionError::Readiness)?
            {
                CustodyClass::Retained(_) => {}
                CustodyClass::Pending => {
                    return Err(NativeAdmissionError::Readiness(
                        ControllerReadinessFailure::OriginalAdmissionPending,
                    ));
                }
                CustodyClass::Cancelled => return Err(refused(OperationRefusalKind::Denied)),
                CustodyClass::NotRetained => {
                    return Err(refused(OperationRefusalKind::MalformedFacts));
                }
            }
            let custody = self
                .originals
                .authenticated_original(ingress, &claims.ingress_namespace, contributor)
                .map_err(NativeAdmissionError::Readiness)?
                .ok_or_else(|| refused(OperationRefusalKind::MalformedFacts))?;
            if &custody.original_work != contributor {
                return Err(refused(OperationRefusalKind::MalformedFacts));
            }
            self.require_active_admission(&claims.ingress_namespace, contributor)?;
            require_contributor_live(&self.grants, &custody.authority_basis)?;
        }
        Ok(())
    }

    fn require_active_admission(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<(), NativeAdmissionError> {
        match self
            .originals
            .original_disposition(namespace, original)
            .map_err(NativeAdmissionError::Readiness)?
        {
            OriginalDisposition::Active => Ok(()),
            OriginalDisposition::OperationCancelled { .. } => {
                Err(refused(OperationRefusalKind::Denied))
            }
        }
    }

    /// The admitted original's own grant-lineage mandate must resolve in the
    /// actual grant owner for the association's executor and represented
    /// subject. Other bases are the host invocation policy's, already
    /// consulted.
    fn resolve_basis(
        &self,
        basis: &WorkAuthorityBasis,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        let claims = association.candidate();
        resolve_original_basis(
            &self.grants,
            basis,
            &claims.logical_executor,
            claims.represented_subject.as_ref(),
        )
    }
}

/// Liveness only: whether `original` is, now, an original its source owner
/// still retains with an active disposition, and whose own single
/// grant-lineage mandate (if it has one) resolves for
/// `executor`/`represented_subject` in the grant owner. Reads the source
/// owner's custody, never a submitted claim.
///
/// This is not a permission for any operation. It selects no operation
/// target, so a targeted mandate's per-target lineages are not resolved
/// here, and it applies no operation ceiling: a caller authorizing a
/// particular operation must compose its own current permission for that
/// operation. Call it while owner changes are excluded
/// ([`LocalGrantAuthority::try_with_owner_changes_excluded`]) to gate a host
/// commit on the original still being live.
///
/// # Errors
/// A source owner that no longer retains the original, or reports it
/// cancelled, refuses; a source owner that cannot say now is unavailable,
/// and one whose admission of the original is still pending is not yet
/// ready ([`ControllerReadinessFailure::OriginalAdmissionPending`]).
pub fn require_source_original_live(
    grants: &LocalGrantAuthority,
    originals: &dyn AuthenticatedOriginalSource,
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
    executor: &PrincipalRef,
    represented_subject: Option<&PrincipalRef>,
) -> Result<(), NativeAdmissionError> {
    match originals
        .original_disposition(namespace, original)
        .map_err(NativeAdmissionError::Readiness)?
    {
        OriginalDisposition::Active => {}
        OriginalDisposition::OperationCancelled { .. } => {
            return Err(refused(OperationRefusalKind::Denied));
        }
    }
    let basis = match classify_custody(originals, namespace, original)
        .map_err(NativeAdmissionError::Readiness)?
    {
        CustodyClass::Retained(basis) => basis,
        CustodyClass::Pending => {
            return Err(NativeAdmissionError::Readiness(
                ControllerReadinessFailure::OriginalAdmissionPending,
            ));
        }
        CustodyClass::Cancelled | CustodyClass::NotRetained => {
            return Err(refused(OperationRefusalKind::Denied));
        }
    };
    resolve_original_basis(grants, &basis, executor, represented_subject)
}

/// A grant-lineage mandate must resolve in the actual grant owner for the
/// executor and represented subject. Other bases are the host invocation
/// policy's, consulted by its own owner.
fn resolve_original_basis(
    grants: &LocalGrantAuthority,
    basis: &WorkAuthorityBasis,
    executor: &PrincipalRef,
    represented_subject: Option<&PrincipalRef>,
) -> Result<(), NativeAdmissionError> {
    for lineage in meerkat_authorization::grant_policy::admission_grant_lineages(basis) {
        grants
            .resolve_lineage(lineage, executor, represented_subject)
            .map_err(grant_refusal)?;
    }
    Ok(())
}

/// Liveness only, for a contributor an admitted original names: its own
/// grant-lineage mandate is current in the grant owner for that lineage's
/// own leaf grantee and represented subject
/// ([`LocalGrantAuthority::require_live_lineage`]). A contributor may be
/// another executor's original (a helper names its source's original), so
/// it is not resolved for the admitted executor here. This permits no
/// operation: each operation of the work is narrowed by every contributor's
/// current ceiling ([`contributor_operation_ceiling`]) and still requires the
/// work's own authority. Other bases are the host invocation policy's,
/// consulted by its own owner.
pub(super) fn require_contributor_live(
    grants: &LocalGrantAuthority,
    basis: &WorkAuthorityBasis,
) -> Result<(), NativeAdmissionError> {
    for lineage in meerkat_authorization::grant_policy::admission_grant_lineages(basis) {
        grants
            .require_live_lineage(lineage)
            .map_err(grant_refusal)?;
    }
    Ok(())
}

/// Bind one authenticated original to its final governed input: set the
/// association and the original event's raw idempotency key, then bind the
/// trusted ingress observation and the actual selected controller to exactly
/// that input. The ingress context retains the input id and the replay digest
/// of this final input, so any later change of it is refused by the existing
/// submission check. The raw key is qualified by the association at admission
/// and only selects a replay candidate; it is the replay digest that decides
/// replay versus conflict.
#[allow(clippy::too_many_arguments)]
pub fn bind_scoped_original(
    mut input: Input,
    association: InputAuthorityAssociation,
    event_key: IdempotencyKey,
    requester: PrincipalRef,
    ingress_actor: PrincipalRef,
    realm: meerkat_core::connection::RealmId,
    authentication: HistoricalEvidenceRef,
    controller: meerkat_core::ControllerModelClient,
) -> Result<Input, RuntimeDriverError> {
    input.header_mut().authority_association = Some(association);
    input.header_mut().idempotency_key = Some(event_key);
    let ingress = NativeIngressContext::from_trusted_ingress(
        &input,
        requester,
        ingress_actor,
        realm,
        authentication,
    )?
    .with_controller_client(&input, controller)?;
    input.with_ingress_context(ingress)
}

fn refused(kind: OperationRefusalKind) -> NativeAdmissionError {
    NativeAdmissionError::Refused(OperationRefused::new(kind))
}

fn grant_refusal(refusal: GrantRefusal) -> NativeAdmissionError {
    match refusal {
        GrantRefusal::Denied => refused(OperationRefusalKind::Denied),
        GrantRefusal::ControllerInUse | GrantRefusal::Duplicate | GrantRefusal::Unavailable => {
            NativeAdmissionError::Readiness(ControllerReadinessFailure::PolicyUnavailable)
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests;
