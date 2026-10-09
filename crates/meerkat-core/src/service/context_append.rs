//! Exact, process-only admission for one external System-context append.

use std::any::Any;
use std::sync::Arc;

use crate::approval::review::OperationReviewRefusal;
use crate::authorization::{
    AuthorizationOperation, ObservedAuthorizationResult, OperationAuthorizationError,
    OperationAuthorizationFacts, OwnerQualifiedTarget, PolicyPublicationObservation,
    PreparedAuthorizationBinding, PreparedOperationCheck, PublicationAuthorizationFacts,
    PublicationMode, PublicationRecipient, SourceAuthorizationTarget, WorkAuthorizationContext,
};
use crate::exact_operation::OperationExecutionScope;
use crate::session::context_control::{
    ContextControlAuditOutcome, ContextControlAuditRecord, ContextControlAuditSource,
};
use crate::{OperationId, PrincipalRef, Session, SessionId};

use super::{
    AppendSystemContextRequest, AppendSystemContextResult, AppendSystemContextStatus,
    SessionControlError,
};

/// Facts resolved by a trusted control ingress, not authority inferred from the
/// append's `source` annotation or a session's most recent work.
#[derive(Clone)]
pub struct ContextControlFacts {
    pub requester: PrincipalRef,
    pub actor: PrincipalRef,
    pub realm: crate::connection::RealmId,
    pub source: SourceAuthorizationTarget,
    pub destination: OwnerQualifiedTarget,
}

/// Exact immutable control submitted to the installed native owners. The
/// process evidence is a real host/mandate receipt whose concrete type and
/// current validity those owners must verify. There is no serde/wire admission.
pub struct SystemContextControlRequest {
    operation_id: OperationId,
    session_id: SessionId,
    request: AppendSystemContextRequest,
    facts: ContextControlFacts,
    evidence: Arc<dyn Any + Send + Sync>,
    request_digest: String,
    // A control owns one settlement attempt, including a pre-entry refusal.
    // Every preparation of this exact process receipt shares the claim.
    claimed: Arc<std::sync::atomic::AtomicBool>,
}

impl SystemContextControlRequest {
    pub fn from_trusted_ingress<T: Any + Send + Sync>(
        session_id: SessionId,
        request: AppendSystemContextRequest,
        facts: ContextControlFacts,
        evidence: Arc<T>,
    ) -> Result<Arc<Self>, OperationAuthorizationError> {
        facts
            .requester
            .validate_qualified()
            .map_err(|_| malformed())?;
        facts.actor.validate_qualified().map_err(|_| malformed())?;
        facts
            .destination
            .authority
            .validate_qualified()
            .map_err(|_| malformed())?;
        let request_digest = ContextControlAuditRecord::digest_request(&request)?;
        Ok(Arc::new(Self {
            operation_id: OperationId::new(),
            session_id,
            request,
            facts,
            evidence,
            request_digest,
            claimed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }))
    }

    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    pub fn request(&self) -> &AppendSystemContextRequest {
        &self.request
    }
    pub fn facts(&self) -> &ContextControlFacts {
        &self.facts
    }
    pub fn evidence<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.evidence.downcast_ref()
    }
}

struct BoundSystemContextAppend {
    owner: Arc<dyn Any + Send + Sync>,
    control: Arc<SystemContextControlRequest>,
    check: PreparedOperationCheck,
}

/// Native-owner-prepared request. A single check conjunctively covers actual
/// invocation, source access and destination publication at the same policy
/// observation. It is not permission for another payload or owner.
#[derive(Clone)]
pub struct PreparedSystemContextAppend(Arc<BoundSystemContextAppend>);

impl PreparedSystemContextAppend {
    /// Trusted native composition; `context` must resolve every control fact
    /// through the configured admission and source/destination policy owners.
    pub fn prepare<T: Any + Send + Sync>(
        owner: Arc<T>,
        control: Arc<SystemContextControlRequest>,
        context: WorkAuthorizationContext,
    ) -> Result<Self, OperationAuthorizationError> {
        Self::prepare_observed(owner, control, context).result
    }

    pub fn prepare_observed<T: Any + Send + Sync>(
        owner: Arc<T>,
        control: Arc<SystemContextControlRequest>,
        context: WorkAuthorizationContext,
    ) -> ObservedAuthorizationResult<Self> {
        if context.execution_scope() != &OperationExecutionScope::Domain {
            return ObservedAuthorizationResult::unobserved(Err(malformed()));
        }
        PreparedOperationCheck::prepare_observed(
            context,
            PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
                operation_id: control.operation_id.clone(),
                execution_scope: OperationExecutionScope::Domain,
                run_id: None,
                context_revision: None,
                operation: AuthorizationOperation::Publication(PublicationAuthorizationFacts {
                    recipient: PublicationRecipient::Destination(control.facts.destination.clone()),
                    scope: None,
                    live_channel: None,
                    mode: PublicationMode::Buffered,
                }),
            }),
        )
        .map(|check| {
            Self(Arc::new(BoundSystemContextAppend {
                owner,
                control,
                check,
            }))
        })
    }

    pub fn session_id(&self) -> &SessionId {
        self.0.control.session_id()
    }
    pub fn request(&self) -> &AppendSystemContextRequest {
        self.0.control.request()
    }
    pub fn control(&self) -> &Arc<SystemContextControlRequest> {
        &self.0.control
    }

    /// Compare the actual native owner, not a serialized session/owner label.
    pub fn belongs_to<T: Any + Send + Sync>(&self, owner: &Arc<T>) -> bool {
        Arc::as_ptr(&self.0.owner).cast::<()>() == Arc::as_ptr(owner).cast::<()>()
    }

    /// Currentness (source access and destination audience under one
    /// observation) first, then the current decision's review tier, then the
    /// entry observation. Context controls carry no operation review, so a
    /// current R2/R3 decision settles as typed review feedback with no entry
    /// observation and no mutation. The entry observer is arbitrary owner
    /// code that may synchronize with or change the owners, so currentness
    /// and the tier are checked AGAIN after it (mirroring the provider HTTP
    /// entry); the refreshed decision and its publication are what the
    /// append, its outcome and its audit carry forward.
    fn enter_observed(
        &self,
        session_id: &SessionId,
        request: &AppendSystemContextRequest,
    ) -> ObservedAuthorizationResult<AppendAdmission> {
        if session_id != self.session_id() || !std::ptr::eq(request, self.request()) {
            return ObservedAuthorizationResult::unobserved(Err(malformed()));
        }
        let observed = self.0.check.current_observed();
        let check = match observed.result {
            Ok(check) => check,
            Err(error) => {
                return ObservedAuthorizationResult {
                    result: Err(error),
                    policy: observed.policy,
                };
            }
        };
        if let Err(refusal) = check.require_unreviewed_entry() {
            return ObservedAuthorizationResult {
                result: Ok(AppendAdmission::ReviewRefused(refusal)),
                policy: observed.policy,
            };
        }
        if let Err(error) = check.observe_entry() {
            return ObservedAuthorizationResult {
                result: Err(error.into()),
                policy: observed.policy,
            };
        }
        let refreshed = check.current_observed();
        ObservedAuthorizationResult {
            result: refreshed
                .result
                .map(|current| match current.require_unreviewed_entry() {
                    Ok(()) => AppendAdmission::Entered(current),
                    Err(refusal) => AppendAdmission::ReviewRefused(refusal),
                }),
            policy: refreshed.policy,
        }
    }
}

/// A current, permitted decision either entered or settled its required
/// review locally. Review settlement is neither a permission refusal nor an
/// observation failure.
enum AppendAdmission {
    Entered(PreparedOperationCheck),
    ReviewRefused(OperationReviewRefusal),
}

/// Session-owner command retained across actor and persistence waits. Missing
/// ingress is representable for audit but never becomes permission. Services
/// must route this command only through the matching canonical runtime owner.
#[derive(Clone)]
pub struct SystemContextAppendControl {
    operation_id: OperationId,
    session_id: SessionId,
    request: AppendSystemContextRequest,
    request_digest: String,
    prepared: Option<PreparedSystemContextAppend>,
    control: Option<Arc<SystemContextControlRequest>>,
    failure: Option<OperationAuthorizationError>,
    preparation_policy: Option<PolicyPublicationObservation>,
    claimed: Arc<std::sync::atomic::AtomicBool>,
}

impl SystemContextAppendControl {
    pub fn unavailable(
        session_id: SessionId,
        request: AppendSystemContextRequest,
    ) -> Result<Self, OperationAuthorizationError> {
        let request_digest = ContextControlAuditRecord::digest_request(&request)?;
        Ok(Self {
            operation_id: OperationId::new(),
            session_id,
            request,
            request_digest,
            prepared: None,
            control: None,
            failure: Some(OperationAuthorizationError::Unavailable),
            preparation_policy: None,
            claimed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    pub fn authorized(prepared: PreparedSystemContextAppend) -> Self {
        Self {
            operation_id: prepared.0.control.operation_id.clone(),
            session_id: prepared.session_id().clone(),
            request: prepared.request().clone(),
            request_digest: prepared.0.control.request_digest.clone(),
            claimed: Arc::clone(&prepared.0.control.claimed),
            control: Some(Arc::clone(prepared.control())),
            prepared: Some(prepared),
            failure: None,
            preparation_policy: None,
        }
    }

    pub fn from_preparation(
        control: Arc<SystemContextControlRequest>,
        prepared: Result<PreparedSystemContextAppend, OperationAuthorizationError>,
    ) -> Self {
        Self::from_observed_preparation(control, ObservedAuthorizationResult::unobserved(prepared))
    }

    pub fn from_observed_preparation(
        control: Arc<SystemContextControlRequest>,
        prepared: ObservedAuthorizationResult<PreparedSystemContextAppend>,
    ) -> Self {
        let policy = prepared.policy;
        match prepared.result {
            Ok(prepared) if Arc::ptr_eq(&control, prepared.control()) => Self::authorized(prepared),
            prepared => Self {
                operation_id: control.operation_id.clone(),
                session_id: control.session_id.clone(),
                request: control.request.clone(),
                request_digest: control.request_digest.clone(),
                claimed: Arc::clone(&control.claimed),
                preparation_policy: if prepared.is_err() { policy } else { None },
                failure: Some(prepared.err().unwrap_or_else(malformed)),
                control: Some(control),
                prepared: None,
            },
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Synchronous final owner boundary. The service must commit the resulting
    /// Session even when this returns an authorization error: denied controls
    /// have an observation, but no appended message. Audit failure is distinct
    /// from permission refusal and never authorizes replay after an effect.
    pub fn apply(
        &self,
        session: &mut Session,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        if session.id() != self.session_id() {
            return Err(malformed().into());
        }
        // Claim before either authorization settlement or physical mutation.
        // A new attempt needs a new operation; transcript idempotency still
        // decides whether a newly authorized attempt is an exact duplicate.
        self.claimed
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .map_err(|_| malformed())?;
        let admission = match &self.prepared {
            Some(prepared) => prepared.enter_observed(session.id(), prepared.request()),
            None => ObservedAuthorizationResult {
                result: Err(self
                    .failure
                    .unwrap_or(OperationAuthorizationError::Unavailable)),
                policy: self.preparation_policy,
            },
        };
        let policy = admission.policy;
        let check = match admission.result {
            Ok(AppendAdmission::Entered(check)) => check,
            Ok(AppendAdmission::ReviewRefused(refusal)) => {
                session
                    .record_context_control_observation(self.record(
                        ContextControlAuditOutcome::ReviewRefused { refusal },
                        policy,
                    ))
                    .map_err(OperationAuthorizationError::from)?;
                return Err(SessionControlError::Review(refusal));
            }
            Err(error) => {
                let outcome = match error {
                    OperationAuthorizationError::Refused(reason) => {
                        ContextControlAuditOutcome::Refused {
                            reason: reason.kind(),
                        }
                    }
                    OperationAuthorizationError::Unavailable => {
                        ContextControlAuditOutcome::AuthorizationUnavailable
                    }
                    OperationAuthorizationError::ObservationUnavailable(_) => {
                        return Err(error.into());
                    }
                };
                session
                    .record_context_control_observation(self.record(outcome, policy))
                    .map_err(OperationAuthorizationError::from)?;
                return Err(error.into());
            }
        };
        let index = session.messages().len();
        let request = self
            .prepared
            .as_ref()
            .map_or(&self.request, |p| p.request());
        let appended = session.append_system_message_control_idempotent(
            request.text(),
            request.source.clone(),
            request.idempotency_key.clone(),
            crate::types::message_timestamp_now(),
        );
        let status = match appended {
            Ok(status) => status,
            Err(error) => {
                let observation = check.observe_outcome(
                    crate::authorization::OperationObservedOutcome::ContextAppendError,
                );
                session
                    .record_context_control_observation(
                        self.record(ContextControlAuditOutcome::AppendError, policy),
                    )
                    .map_err(OperationAuthorizationError::from)?;
                observation.map_err(OperationAuthorizationError::from)?;
                return Err(error.into_control_error(session.id()));
            }
        };
        let outcome = match status {
            AppendSystemContextStatus::Applied => {
                let Some(crate::Message::System(message)) = session.messages().get(index) else {
                    return Err(OperationAuthorizationError::ObservationUnavailable(
                        crate::authorization::OperationObservationError,
                    )
                    .into());
                };
                let identity = message.identity.clone();
                ContextControlAuditOutcome::Appended {
                    message_index: index as u64,
                    identity,
                }
            }
            AppendSystemContextStatus::Duplicate => ContextControlAuditOutcome::Duplicate,
        };
        session
            .record_context_control_observation(self.record(outcome, policy))
            .map_err(OperationAuthorizationError::from)?;
        check
            .observe_outcome(
                crate::authorization::OperationObservedOutcome::ContextAppendReturned { status },
            )
            .map_err(OperationAuthorizationError::from)?;
        Ok(AppendSystemContextResult { status })
    }

    fn record(
        &self,
        outcome: ContextControlAuditOutcome,
        policy: Option<PolicyPublicationObservation>,
    ) -> ContextControlAuditRecord {
        let facts = self.control.as_ref().map(|control| control.facts());
        ContextControlAuditRecord {
            operation_id: self.operation_id.clone(),
            session_id: self.session_id.clone(),
            requester: facts.map(|f| f.requester.clone()),
            actor: facts.map(|f| f.actor.clone()),
            realm: facts.map(|f| f.realm.clone()),
            source: facts.map(|f| ContextControlAuditSource::from(&f.source)),
            request_digest: self.request_digest.clone(),
            observed_at_ms: crate::time_compat::SystemTime::now()
                .duration_since(crate::time_compat::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            policy: policy.into_iter().collect(),
            outcome,
        }
    }
}

macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => {$ (
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($ty), "([REDACTED])"))
            }
        }
    )+ };
}
redacted_debug!(
    ContextControlFacts,
    SystemContextControlRequest,
    PreparedSystemContextAppend,
    SystemContextAppendControl
);

fn malformed() -> OperationAuthorizationError {
    crate::OperationRefused::new(crate::OperationRefusalKind::MalformedFacts).into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::authorization::{
        ObservedAuthorizationResult, PolicyPublicationObservation, PreparedOperationAuthorization,
        WorkAuthorization, WorkAuthorizationContext,
    };

    struct ObservedOwner {
        instance: uuid::Uuid,
        sequence: std::sync::atomic::AtomicU64,
        allowed: AtomicBool,
        unavailable: AtomicBool,
        preparations: AtomicUsize,
        entries: AtomicUsize,
    }

    struct ObservedDecision {
        owner: Arc<ObservedOwner>,
        binding: PreparedAuthorizationBinding,
        sequence: u64,
    }

    impl WorkAuthorization for Arc<ObservedOwner> {
        fn prepare(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            self.prepare_observed(binding).result
        }

        fn prepare_observed(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> ObservedAuthorizationResult<Arc<dyn PreparedOperationAuthorization>> {
            self.preparations.fetch_add(1, Ordering::Relaxed);
            let sequence = self.sequence.load(Ordering::Acquire);
            let result = if self.allowed.load(Ordering::Acquire) {
                Ok(Arc::new(ObservedDecision {
                    owner: Arc::clone(self),
                    binding: binding.clone(),
                    sequence,
                })
                    as Arc<dyn PreparedOperationAuthorization>)
            } else {
                Err(crate::OperationRefused::new(crate::OperationRefusalKind::Denied).into())
            };
            ObservedAuthorizationResult {
                result,
                policy: Some(PolicyPublicationObservation::LocalPublication {
                    instance: self.instance,
                    sequence,
                }),
            }
        }
    }

    impl PreparedOperationAuthorization for ObservedDecision {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            crate::authorization::OperationReviewTier::R1
        }

        fn policy_observation(&self) -> Option<PolicyPublicationObservation> {
            Some(PolicyPublicationObservation::LocalPublication {
                instance: self.owner.instance,
                sequence: self.sequence,
            })
        }

        fn check_current(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            if self.owner.unavailable.load(Ordering::Acquire) {
                return Err(OperationAuthorizationError::Unavailable);
            }
            if !self.binding.same_operation(binding)
                || self.sequence != self.owner.sequence.load(Ordering::Acquire)
            {
                return Err(crate::OperationRefused::new(
                    crate::OperationRefusalKind::ReprepareRequired,
                )
                .into());
            }
            Ok(())
        }

        fn observe(
            &self,
            _: &PreparedAuthorizationBinding,
            observation: crate::authorization::OperationObservation,
        ) -> Result<(), crate::authorization::OperationObservationError> {
            if matches!(
                observation,
                crate::authorization::OperationObservation::Entry
            ) {
                self.owner.entries.fetch_add(1, Ordering::AcqRel);
            }
            Ok(())
        }
    }

    fn observed_control(session: &Session) -> Arc<SystemContextControlRequest> {
        let permitted = Arc::new(AtomicBool::new(true));
        let entries = Arc::new(AtomicUsize::new(0));
        fixture(
            session.id().clone(),
            AppendSystemContextRequest::from_text("authorized context"),
            owner(&permitted, &entries),
            owner(&permitted, &entries),
        )
        .control()
        .clone()
    }

    #[test]
    fn refreshed_context_decision_audits_its_own_publication_on_allow_and_deny() {
        for allowed_after_change in [true, false] {
            let mut session = Session::new();
            let control = observed_control(&session);
            let owner = Arc::new(ObservedOwner {
                instance: uuid::Uuid::new_v4(),
                sequence: std::sync::atomic::AtomicU64::new(0),
                allowed: AtomicBool::new(true),
                unavailable: AtomicBool::new(false),
                preparations: AtomicUsize::new(0),
                entries: AtomicUsize::new(0),
            });
            let prepared = PreparedSystemContextAppend::prepare(
                Arc::new(()),
                Arc::clone(&control),
                WorkAuthorizationContext::new(
                    Arc::new(Arc::clone(&owner)),
                    OperationExecutionScope::Domain,
                ),
            )
            .unwrap();
            owner.allowed.store(allowed_after_change, Ordering::Release);
            owner.sequence.store(2, Ordering::Release);
            let result = SystemContextAppendControl::authorized(prepared).apply(&mut session);
            if allowed_after_change {
                assert_eq!(result.unwrap().status, AppendSystemContextStatus::Applied);
            } else {
                assert!(
                    matches!(result, Err(SessionControlError::Authorization(OperationAuthorizationError::Refused(reason))) if reason.kind() == crate::OperationRefusalKind::Denied)
                );
            }
            let record = session
                .context_control_observation(control.operation_id())
                .unwrap()
                .unwrap();
            assert_eq!(
                record.policy,
                vec![PolicyPublicationObservation::LocalPublication {
                    instance: owner.instance,
                    sequence: 2
                }]
            );
            assert_eq!(owner.preparations.load(Ordering::Relaxed), 2);
            assert_eq!(
                owner.entries.load(Ordering::Relaxed),
                usize::from(allowed_after_change)
            );
            assert_eq!(session.messages().len(), usize::from(allowed_after_change));
        }
    }

    #[test]
    fn coherent_preparation_denial_reaches_session_audit_without_entry() {
        let mut session = Session::new();
        let control = observed_control(&session);
        let owner = Arc::new(ObservedOwner {
            instance: uuid::Uuid::new_v4(),
            sequence: std::sync::atomic::AtomicU64::new(4),
            allowed: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            preparations: AtomicUsize::new(0),
            entries: AtomicUsize::new(0),
        });
        let observed = PreparedSystemContextAppend::prepare_observed(
            Arc::new(()),
            Arc::clone(&control),
            WorkAuthorizationContext::new(
                Arc::new(Arc::clone(&owner)),
                OperationExecutionScope::Domain,
            ),
        );
        let result =
            SystemContextAppendControl::from_observed_preparation(Arc::clone(&control), observed)
                .apply(&mut session);
        assert!(
            matches!(result, Err(SessionControlError::Authorization(OperationAuthorizationError::Refused(reason))) if reason.kind() == crate::OperationRefusalKind::Denied)
        );
        let record = session
            .context_control_observation(control.operation_id())
            .unwrap()
            .unwrap();
        assert_eq!(
            record.policy,
            vec![PolicyPublicationObservation::LocalPublication {
                instance: owner.instance,
                sequence: 4
            }]
        );
        assert_eq!(owner.entries.load(Ordering::Relaxed), 0);
        assert!(session.messages().is_empty());
    }

    #[test]
    fn unavailable_current_owner_does_not_reuse_the_old_policy_observation() {
        let mut session = Session::new();
        let control = observed_control(&session);
        let owner = Arc::new(ObservedOwner {
            instance: uuid::Uuid::new_v4(),
            sequence: std::sync::atomic::AtomicU64::new(0),
            allowed: AtomicBool::new(true),
            unavailable: AtomicBool::new(false),
            preparations: AtomicUsize::new(0),
            entries: AtomicUsize::new(0),
        });
        let prepared = PreparedSystemContextAppend::prepare(
            Arc::new(()),
            Arc::clone(&control),
            WorkAuthorizationContext::new(
                Arc::new(Arc::clone(&owner)),
                OperationExecutionScope::Domain,
            ),
        )
        .unwrap();
        owner.unavailable.store(true, Ordering::Release);
        let result = SystemContextAppendControl::authorized(prepared).apply(&mut session);
        assert!(matches!(
            result,
            Err(SessionControlError::Authorization(
                OperationAuthorizationError::Unavailable
            ))
        ));
        let record = session
            .context_control_observation(control.operation_id())
            .unwrap()
            .unwrap();
        assert!(record.policy.is_empty());
        assert_eq!(
            record.outcome,
            ContextControlAuditOutcome::AuthorizationUnavailable
        );
        assert_eq!(owner.preparations.load(Ordering::Relaxed), 1);
        assert_eq!(owner.entries.load(Ordering::Relaxed), 0);
        assert!(session.messages().is_empty());
    }

    struct Owner {
        permitted: Arc<AtomicBool>,
        entries: Arc<AtomicUsize>,
        review_tier: crate::authorization::OperationReviewTier,
    }

    impl WorkAuthorization for Owner {
        fn prepare(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(Decision {
                binding: binding.clone(),
                permitted: Arc::clone(&self.permitted),
                entries: Arc::clone(&self.entries),
                review_tier: self.review_tier,
            }))
        }
    }

    struct Decision {
        binding: PreparedAuthorizationBinding,
        permitted: Arc<AtomicBool>,
        entries: Arc<AtomicUsize>,
        review_tier: crate::authorization::OperationReviewTier,
    }

    impl PreparedOperationAuthorization for Decision {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            self.review_tier
        }

        fn check_current(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            if !self.binding.same_operation(binding) || !self.permitted.load(Ordering::Acquire) {
                return Err(
                    crate::OperationRefused::new(crate::OperationRefusalKind::Denied).into(),
                );
            }
            Ok(())
        }

        fn observe(
            &self,
            _binding: &PreparedAuthorizationBinding,
            observation: crate::authorization::OperationObservation,
        ) -> Result<(), crate::authorization::OperationObservationError> {
            if matches!(
                observation,
                crate::authorization::OperationObservation::Entry
            ) {
                self.entries.fetch_add(1, Ordering::AcqRel);
            }
            Ok(())
        }
    }

    fn owner(permitted: &Arc<AtomicBool>, entries: &Arc<AtomicUsize>) -> WorkAuthorizationContext {
        owner_with_review_tier(
            permitted,
            entries,
            crate::authorization::OperationReviewTier::R1,
        )
    }

    fn owner_with_review_tier(
        permitted: &Arc<AtomicBool>,
        entries: &Arc<AtomicUsize>,
        review_tier: crate::authorization::OperationReviewTier,
    ) -> WorkAuthorizationContext {
        WorkAuthorizationContext::new(
            Arc::new(Owner {
                permitted: Arc::clone(permitted),
                entries: Arc::clone(entries),
                review_tier,
            }),
            crate::exact_operation::OperationExecutionScope::Domain,
        )
    }

    fn fixture(
        session_id: SessionId,
        request: AppendSystemContextRequest,
        source: WorkAuthorizationContext,
        destination: WorkAuthorizationContext,
    ) -> PreparedSystemContextAppend {
        let principal = crate::PrincipalRef::in_domain(
            crate::PrincipalKind::ServiceAccount,
            "context-owner",
            crate::TrustDomainId::new("test-context-domain").unwrap(),
        )
        .unwrap();
        let target = OwnerQualifiedTarget {
            authority: principal.clone(),
            namespace: Arc::from("session-context"),
            id: Arc::from(session_id.to_string()),
        };
        let control = SystemContextControlRequest::from_trusted_ingress(
            session_id,
            request,
            ContextControlFacts {
                requester: principal.clone(),
                actor: principal,
                realm: crate::connection::RealmId::global(),
                source: SourceAuthorizationTarget::External(target.clone()),
                destination: target,
            },
            Arc::new(()),
        )
        .unwrap();
        struct Both(WorkAuthorizationContext, WorkAuthorizationContext);
        impl WorkAuthorization for Both {
            fn prepare(
                &self,
                binding: &PreparedAuthorizationBinding,
            ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError>
            {
                struct Decision(
                    Arc<dyn PreparedOperationAuthorization>,
                    Arc<dyn PreparedOperationAuthorization>,
                );
                impl PreparedOperationAuthorization for Decision {
                    // The conjunction carries the strictest of its inner
                    // owners' tiers, never a fixed tier.
                    fn review_tier(&self) -> crate::authorization::OperationReviewTier {
                        self.0.review_tier().max(self.1.review_tier())
                    }

                    fn check_current(
                        &self,
                        binding: &PreparedAuthorizationBinding,
                    ) -> Result<(), OperationAuthorizationError> {
                        self.0.check_current(binding)?;
                        self.1.check_current(binding)
                    }
                    fn observe(
                        &self,
                        binding: &PreparedAuthorizationBinding,
                        observation: crate::authorization::OperationObservation,
                    ) -> Result<(), crate::authorization::OperationObservationError>
                    {
                        self.0.observe(binding, observation.clone())?;
                        self.1.observe(binding, observation)
                    }
                }
                Ok(Arc::new(Decision(
                    self.0.authorization().prepare(binding)?,
                    self.1.authorization().prepare(binding)?,
                )))
            }
        }
        PreparedSystemContextAppend::prepare(
            Arc::new(()),
            control,
            WorkAuthorizationContext::new(
                Arc::new(Both(source, destination)),
                OperationExecutionScope::Domain,
            ),
        )
        .unwrap()
    }

    /// Required review on either owner must prevent this direct native append.
    /// There is no reviewer or human decision in this fixture. A later R1
    /// operation on the same Session must still be able to append.
    #[test]
    fn context_append_cannot_ignore_required_source_or_destination_review() {
        use crate::authorization::OperationReviewTier::{R1, R2, R3};

        for (source_tier, destination_tier) in [(R2, R1), (R1, R2), (R3, R1), (R1, R3)] {
            let allowed = Arc::new(AtomicBool::new(true));
            let entries = Arc::new(AtomicUsize::new(0));
            let mut session = Session::with_id(SessionId::new());
            let prepared = fixture(
                session.id().clone(),
                AppendSystemContextRequest::from_text("requires review"),
                owner_with_review_tier(&allowed, &entries, source_tier),
                owner_with_review_tier(&allowed, &entries, destination_tier),
            );
            assert_eq!(
                prepared.0.check.review_tier(),
                source_tier.max(destination_tier),
            );

            let operation_id = prepared.control().operation_id().clone();
            let result = SystemContextAppendControl::authorized(prepared).apply(&mut session);
            let expected = match source_tier.max(destination_tier) {
                R2 => crate::approval::review::OperationReviewRefusal::Unavailable {
                    kind: crate::approval::review::ReviewUnavailableKind::UnsupportedEntry,
                },
                _ => crate::approval::review::OperationReviewRefusal::Unsatisfied {
                    kind: crate::approval::review::ReviewUnsatisfiedKind::HumanConsentRequired,
                },
            };
            assert!(
                matches!(&result, Err(SessionControlError::Review(refusal)) if *refusal == expected),
                "unsatisfied source {source_tier:?} / destination {destination_tier:?}: {result:?}",
            );
            assert_eq!(entries.load(Ordering::Acquire), 0);
            assert!(session.messages().is_empty());
            // Recorded as typed review feedback, not a permission refusal.
            assert_eq!(
                session
                    .context_control_observation(&operation_id)
                    .unwrap()
                    .unwrap()
                    .outcome,
                ContextControlAuditOutcome::ReviewRefused { refusal: expected },
            );

            let permitted = fixture(
                session.id().clone(),
                AppendSystemContextRequest::from_text("permitted sibling"),
                owner(&allowed, &entries),
                owner(&allowed, &entries),
            );
            assert_eq!(
                SystemContextAppendControl::authorized(permitted)
                    .apply(&mut session)
                    .unwrap()
                    .status,
                AppendSystemContextStatus::Applied,
            );
            assert_eq!(entries.load(Ordering::Acquire), 2);
            assert_eq!(session.messages().len(), 1);
        }
    }

    #[test]
    fn context_append_checks_both_current_source_and_destination_before_entry() {
        let source_allowed = Arc::new(AtomicBool::new(true));
        let destination_allowed = Arc::new(AtomicBool::new(true));
        let entries = Arc::new(AtomicUsize::new(0));
        let session = SessionId::new();
        let request = AppendSystemContextRequest::from_text("owned context");
        let prepared = fixture(
            session.clone(),
            request,
            owner(&source_allowed, &entries),
            owner(&destination_allowed, &entries),
        );

        source_allowed.store(false, Ordering::Release);
        assert!(
            prepared
                .enter_observed(&session, prepared.request())
                .result
                .is_err()
        );
        assert_eq!(entries.load(Ordering::Acquire), 0);
        source_allowed.store(true, Ordering::Release);
        destination_allowed.store(false, Ordering::Release);
        assert!(
            prepared
                .enter_observed(&session, prepared.request())
                .result
                .is_err()
        );
        assert_eq!(entries.load(Ordering::Acquire), 0);
        destination_allowed.store(true, Ordering::Release);
        assert!(
            prepared
                .enter_observed(&session, prepared.request())
                .result
                .is_ok()
        );
        assert_eq!(entries.load(Ordering::Acquire), 2);
    }

    #[test]
    fn context_append_cannot_retarget_or_replace_the_bound_request() {
        let permitted = Arc::new(AtomicBool::new(true));
        let entries = Arc::new(AtomicUsize::new(0));
        let session = SessionId::new();
        let request = AppendSystemContextRequest::from_text("owned context");
        let prepared = fixture(
            session.clone(),
            request.clone(),
            owner(&permitted, &entries),
            owner(&permitted, &entries),
        );
        assert!(
            prepared
                .enter_observed(&SessionId::new(), &request)
                .result
                .is_err()
        );
        assert!(
            prepared
                .enter_observed(
                    &session,
                    &AppendSystemContextRequest::from_text("other context")
                )
                .result
                .is_err()
        );
        let mut changed = request.clone();
        changed.source = Some("a different source label".to_string());
        assert!(prepared.enter_observed(&session, &changed).result.is_err());
        changed = request;
        changed.idempotency_key = Some("a different delivery".to_string());
        assert!(prepared.enter_observed(&session, &changed).result.is_err());
        assert_eq!(entries.load(Ordering::Acquire), 0);
    }
    #[test]
    fn missing_context_authority_records_unavailable_without_a_message() {
        let id = SessionId::new();
        let mut session = Session::with_id(id.clone());
        let before = session.messages().to_vec();
        let control = SystemContextAppendControl::unavailable(
            id,
            AppendSystemContextRequest::from_text("untrusted notification"),
        )
        .unwrap();
        assert!(matches!(
            control.apply(&mut session),
            Err(SessionControlError::Authorization(
                OperationAuthorizationError::Unavailable
            ))
        ));
        assert_eq!(session.messages(), before);
        let audit = session
            .context_control_observation(&control.operation_id)
            .unwrap()
            .unwrap();
        assert!(matches!(
            audit.outcome,
            ContextControlAuditOutcome::AuthorizationUnavailable
        ));
        assert!(audit.requester.is_none() && audit.source.is_none());
    }

    #[test]
    fn prepared_context_is_one_physical_append_even_when_cloned() {
        let allowed = Arc::new(AtomicBool::new(true));
        let entries = Arc::new(AtomicUsize::new(0));
        let id = SessionId::new();
        let prepared = fixture(
            id.clone(),
            AppendSystemContextRequest::from_text("permitted context"),
            owner(&allowed, &entries),
            owner(&allowed, &entries),
        );
        let mut session = Session::with_id(id);
        let control = SystemContextAppendControl::authorized(prepared);
        assert_eq!(
            control.apply(&mut session).unwrap().status,
            AppendSystemContextStatus::Applied
        );
        let messages = session.messages().to_vec();
        assert!(control.clone().apply(&mut session).is_err());
        assert_eq!(session.messages(), messages);
        assert_eq!(entries.load(Ordering::Acquire), 2);
    }

    #[test]
    fn preparing_the_same_control_twice_does_not_create_another_entry() {
        let allowed = Arc::new(AtomicBool::new(true));
        let entries = Arc::new(AtomicUsize::new(0));
        let id = SessionId::new();
        let first = fixture(
            id.clone(),
            AppendSystemContextRequest::from_text("one operation"),
            owner(&allowed, &entries),
            owner(&allowed, &entries),
        );
        let second = PreparedSystemContextAppend::prepare(
            Arc::new(()),
            Arc::clone(first.control()),
            owner(&allowed, &entries),
        )
        .unwrap();
        let mut session = Session::with_id(id);
        SystemContextAppendControl::authorized(first)
            .apply(&mut session)
            .unwrap();
        let messages = session.messages().to_vec();
        let audit = session
            .context_control_observations()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            SystemContextAppendControl::authorized(second)
                .apply(&mut session)
                .is_err()
        );
        assert_eq!(session.messages(), messages);
        assert_eq!(
            session
                .context_control_observations()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            audit
        );
        assert_eq!(entries.load(Ordering::Acquire), 2);
    }

    #[test]
    fn concurrent_control_clones_share_one_settlement_claim() {
        let allowed = Arc::new(AtomicBool::new(true));
        let entries = Arc::new(AtomicUsize::new(0));
        let id = SessionId::new();
        let prepared = fixture(
            id.clone(),
            AppendSystemContextRequest::from_text("one concurrent operation"),
            owner(&allowed, &entries),
            owner(&allowed, &entries),
        );
        let control = SystemContextAppendControl::authorized(prepared);
        let session = Arc::new(std::sync::Mutex::new(Session::with_id(id)));
        let start = Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let control = control.clone();
                let session = Arc::clone(&session);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    control.apply(&mut session.lock().unwrap()).is_ok()
                })
            })
            .collect::<Vec<_>>();
        let successes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|succeeded| *succeeded)
            .count();
        assert_eq!(successes, 1);
        let session = session.lock().unwrap();
        assert_eq!(session.messages().len(), 1);
        assert_eq!(
            session
                .context_control_observations()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(entries.load(Ordering::Acquire), 2);
    }

    #[test]
    fn settled_preparation_refusal_retains_ingress_and_cannot_later_enter() {
        let allowed = Arc::new(AtomicBool::new(true));
        let entries = Arc::new(AtomicUsize::new(0));
        let id = SessionId::new();
        let prepared = fixture(
            id.clone(),
            AppendSystemContextRequest::from_text("denied operation"),
            owner(&allowed, &entries),
            owner(&allowed, &entries),
        );
        let control = Arc::clone(prepared.control());
        let denied = SystemContextAppendControl::from_preparation(
            Arc::clone(&control),
            Err(crate::OperationRefused::new(crate::OperationRefusalKind::Denied).into()),
        );
        let mut session = Session::with_id(id);
        assert!(
            matches!(denied.apply(&mut session), Err(SessionControlError::Authorization(OperationAuthorizationError::Refused(reason))) if reason.kind() == crate::OperationRefusalKind::Denied)
        );
        let audit = session
            .context_control_observation(control.operation_id())
            .unwrap()
            .unwrap();
        assert_eq!(audit.requester.as_ref(), Some(&control.facts().requester));
        assert_eq!(audit.actor.as_ref(), Some(&control.facts().actor));
        assert_eq!(audit.realm.as_ref(), Some(&control.facts().realm));
        assert!(audit.source.is_some());
        let reprepared = PreparedSystemContextAppend::prepare(
            Arc::new(()),
            Arc::clone(&control),
            owner(&allowed, &entries),
        )
        .unwrap();
        assert!(
            SystemContextAppendControl::authorized(reprepared)
                .apply(&mut session)
                .is_err()
        );
        assert!(session.messages().is_empty());
        assert_eq!(
            session
                .context_control_observation(control.operation_id())
                .unwrap()
                .unwrap(),
            audit
        );
        assert_eq!(entries.load(Ordering::Acquire), 0);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum EntryChange {
        Unchanged,
        RepublishR3,
        Deny,
    }

    /// An owner whose own Entry observer changes its policy: republishes the
    /// operation as R3, or denies it. A real decision change during entry,
    /// not a synthetic error.
    struct ChangingOwner {
        tier: parking_lot::Mutex<crate::authorization::OperationReviewTier>,
        sequence: std::sync::atomic::AtomicU64,
        permitted: AtomicBool,
        on_entry: parking_lot::Mutex<EntryChange>,
    }

    struct ChangingDecision {
        owner: Arc<ChangingOwner>,
        binding: PreparedAuthorizationBinding,
        tier: crate::authorization::OperationReviewTier,
        sequence: u64,
    }

    impl WorkAuthorization for Arc<ChangingOwner> {
        fn prepare(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            if !self.permitted.load(Ordering::Acquire) {
                return Err(
                    crate::OperationRefused::new(crate::OperationRefusalKind::Denied).into(),
                );
            }
            Ok(Arc::new(ChangingDecision {
                owner: Arc::clone(self),
                binding: binding.clone(),
                tier: *self.tier.lock(),
                sequence: self.sequence.load(Ordering::Acquire),
            }))
        }
    }

    impl PreparedOperationAuthorization for ChangingDecision {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            self.tier
        }

        fn check_current(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            if !self.binding.same_operation(binding)
                || !self.owner.permitted.load(Ordering::Acquire)
            {
                return Err(
                    crate::OperationRefused::new(crate::OperationRefusalKind::Denied).into(),
                );
            }
            if self.sequence != self.owner.sequence.load(Ordering::Acquire) {
                return Err(crate::OperationRefused::new(
                    crate::OperationRefusalKind::ReprepareRequired,
                )
                .into());
            }
            Ok(())
        }

        fn observe(
            &self,
            _: &PreparedAuthorizationBinding,
            observation: crate::authorization::OperationObservation,
        ) -> Result<(), crate::authorization::OperationObservationError> {
            if matches!(
                observation,
                crate::authorization::OperationObservation::Entry
            ) {
                match std::mem::replace(&mut *self.owner.on_entry.lock(), EntryChange::Unchanged) {
                    EntryChange::Unchanged => {}
                    EntryChange::RepublishR3 => {
                        *self.owner.tier.lock() = crate::authorization::OperationReviewTier::R3;
                        self.owner.sequence.fetch_add(1, Ordering::AcqRel);
                    }
                    EntryChange::Deny => self.owner.permitted.store(false, Ordering::Release),
                }
            }
            Ok(())
        }
    }

    fn changing_owner(change: EntryChange) -> (Arc<ChangingOwner>, WorkAuthorizationContext) {
        let owner = Arc::new(ChangingOwner {
            tier: parking_lot::Mutex::new(crate::authorization::OperationReviewTier::R1),
            sequence: std::sync::atomic::AtomicU64::new(0),
            permitted: AtomicBool::new(true),
            on_entry: parking_lot::Mutex::new(change),
        });
        let context = WorkAuthorizationContext::new(
            Arc::new(Arc::clone(&owner)),
            OperationExecutionScope::Domain,
        );
        (owner, context)
    }

    /// The Entry observer is arbitrary owner code: a source or destination
    /// policy change it makes (republished R1 to R3, or a denial) is caught
    /// by the recheck after it, so the old decision never appends. The
    /// unchanged control appends.
    #[test]
    fn a_policy_change_during_entry_observation_prevents_the_append() {
        for change in [
            EntryChange::Unchanged,
            EntryChange::RepublishR3,
            EntryChange::Deny,
        ] {
            for changes_source in [true, false] {
                let (_source_owner, source) = changing_owner(if changes_source {
                    change
                } else {
                    EntryChange::Unchanged
                });
                let (_destination_owner, destination) = changing_owner(if changes_source {
                    EntryChange::Unchanged
                } else {
                    change
                });
                let mut session = Session::with_id(SessionId::new());
                let prepared = fixture(
                    session.id().clone(),
                    AppendSystemContextRequest::from_text("observed entry"),
                    source,
                    destination,
                );
                let operation_id = prepared.control().operation_id().clone();
                let result = SystemContextAppendControl::authorized(prepared).apply(&mut session);
                let outcome = session
                    .context_control_observation(&operation_id)
                    .unwrap()
                    .unwrap()
                    .outcome;
                let side = if changes_source {
                    "source"
                } else {
                    "destination"
                };
                match change {
                    EntryChange::Unchanged => {
                        assert_eq!(
                            result.unwrap().status,
                            AppendSystemContextStatus::Applied,
                            "{side}"
                        );
                        assert_eq!(session.messages().len(), 1);
                        assert!(matches!(
                            outcome,
                            ContextControlAuditOutcome::Appended { .. }
                        ));
                    }
                    EntryChange::RepublishR3 => {
                        let expected =
                            crate::approval::review::OperationReviewRefusal::Unsatisfied {
                                kind: crate::approval::review::ReviewUnsatisfiedKind::HumanConsentRequired,
                            };
                        assert!(
                            matches!(&result, Err(SessionControlError::Review(refusal)) if *refusal == expected),
                            "{side}: {result:?}"
                        );
                        assert!(session.messages().is_empty(), "{side}: no append");
                        assert_eq!(
                            outcome,
                            ContextControlAuditOutcome::ReviewRefused { refusal: expected }
                        );
                    }
                    EntryChange::Deny => {
                        assert!(
                            matches!(&result, Err(SessionControlError::Authorization(OperationAuthorizationError::Refused(reason))) if reason.kind() == crate::OperationRefusalKind::Denied),
                            "{side}: {result:?}"
                        );
                        assert!(session.messages().is_empty(), "{side}: no append");
                        assert_eq!(
                            outcome,
                            ContextControlAuditOutcome::Refused {
                                reason: crate::OperationRefusalKind::Denied
                            }
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn review_refused_audit_outcome_round_trips() {
        let refusal = crate::approval::review::OperationReviewRefusal::Unsatisfied {
            kind: crate::approval::review::ReviewUnsatisfiedKind::HumanConsentRequired,
        };
        let outcome = ContextControlAuditOutcome::ReviewRefused { refusal };
        let wire = serde_json::to_value(&outcome).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "kind": "review_refused",
                "refusal": { "review": "unsatisfied", "kind": "human_consent_required" }
            })
        );
        assert_eq!(
            serde_json::from_value::<ContextControlAuditOutcome>(wire).unwrap(),
            outcome
        );
    }

    #[test]
    fn runtime_input_source_control_persists_its_exact_audit_source() {
        let id = SessionId::new();
        let mut session = Session::with_id(id.clone());
        let principal = crate::PrincipalRef::in_domain(
            crate::PrincipalKind::ServiceAccount,
            "context-owner",
            crate::TrustDomainId::new("test-context-domain").unwrap(),
        )
        .unwrap();
        let destination = OwnerQualifiedTarget {
            authority: principal.clone(),
            namespace: Arc::from("session-context"),
            id: Arc::from(id.to_string()),
        };
        let owner_session_id = SessionId::new();
        let runtime_epoch_id = crate::RuntimeEpochId::new();
        let input_id = crate::InputId::new();
        let control = SystemContextControlRequest::from_trusted_ingress(
            id,
            AppendSystemContextRequest::from_text("retained original input context"),
            ContextControlFacts {
                requester: principal.clone(),
                actor: principal,
                realm: crate::connection::RealmId::global(),
                source: SourceAuthorizationTarget::RuntimeInput {
                    owner_session_id: owner_session_id.clone(),
                    runtime_epoch_id: runtime_epoch_id.clone(),
                    input_id: input_id.clone(),
                },
                destination,
            },
            Arc::new(()),
        )
        .unwrap();
        let control = SystemContextAppendControl::from_observed_preparation(
            control,
            ObservedAuthorizationResult::unobserved(Err(OperationAuthorizationError::Unavailable)),
        );
        assert!(control.apply(&mut session).is_err());
        assert!(session.messages().is_empty());
        let expected = ContextControlAuditSource::RuntimeInput {
            owner_session_id,
            runtime_epoch_id,
            input_id,
        };
        let audit = session
            .context_control_observation(&control.operation_id)
            .unwrap()
            .unwrap();
        assert!(audit.source.as_ref() == Some(&expected));
        let restored: Session =
            serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
        let restored_audit = restored
            .context_control_observation(&control.operation_id)
            .unwrap()
            .unwrap();
        assert!(restored_audit.source == Some(expected));
    }
}
