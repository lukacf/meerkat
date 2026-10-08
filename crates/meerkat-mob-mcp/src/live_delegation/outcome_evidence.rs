//! Owner-issued evidence for a LiveResponses outcome append.
//!
//! The delegated executor's outcome reaches the source session as an ordinary
//! System-context append. The generated bridge machine already proved which
//! terminal the original delegated work reached; this module keeps that proof
//! with the append instead of flattening it into text. The evidence is
//! process-only: it has no serde form and no wire constructor, and it travels
//! as the append's process receipt. It is a historical fact about one exact
//! operation, never current permission: native admission still decides the
//! append.
//!
//! The original request facts here (the bridge request digest and canonical
//! context revision) describe the delegated work. They are distinct from the
//! digest of the append request that carries the outcome.

use std::sync::Arc;

use meerkat_core::exact_operation::ExactOperationIdentity;
use meerkat_core::{LiveBridgeOperationCorrelation, MeerkatExecutionTerminal, SessionId};
use meerkat_runtime::live_execution::{
    LiveBridgeExecutionTerminalReceipt, LiveBridgeOperationAdmission,
    LiveBridgeRecoveredTerminalReceipt, LiveBridgeRecoverySnapshot,
};

/// The original delegated request a recovered terminal belongs to: the live
/// admission when the terminal was recovered on the live path after the
/// channel was revoked, or the durable snapshot on the restart path.
#[derive(Clone, Debug)]
pub(crate) enum LiveResponsesOriginalRequest {
    Admission(Arc<LiveBridgeOperationAdmission>),
    Snapshot(LiveBridgeRecoverySnapshot),
}

impl LiveResponsesOriginalRequest {
    fn session_id(&self) -> &SessionId {
        match self {
            Self::Admission(admission) => admission.session_id(),
            Self::Snapshot(snapshot) => snapshot.session_id(),
        }
    }

    fn operation(&self) -> &ExactOperationIdentity<LiveBridgeOperationCorrelation> {
        match self {
            Self::Admission(admission) => admission.operation(),
            Self::Snapshot(snapshot) => snapshot.operation(),
        }
    }

    fn request_digest(&self) -> &str {
        match self {
            Self::Admission(admission) => admission.request_digest().as_str(),
            Self::Snapshot(snapshot) => snapshot.request_digest(),
        }
    }

    fn canonical_context_revision(&self) -> &str {
        match self {
            Self::Admission(admission) => admission.canonical_context_revision().as_str(),
            Self::Snapshot(snapshot) => snapshot.canonical_context_revision(),
        }
    }
}

/// Why a piece of outcome evidence could not be assembled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LiveResponsesEvidenceError {
    #[error(
        "recovered terminal receipt names a different session or operation than its original request"
    )]
    MismatchedOriginal,
    #[error("durable snapshot has no committed terminal")]
    UncommittedSnapshot,
}

/// Owner-issued evidence that one LiveResponses outcome append reports the
/// exact terminal of its original delegated work.
#[derive(Clone, Debug)]
pub(crate) enum LiveResponsesOutcomeEvidence {
    /// Live path: the generated terminal receipt for the active admission,
    /// cloned before the execution completion consumed it.
    LiveTerminal(LiveBridgeExecutionTerminalReceipt),
    /// The generated receipt for a terminal reconciled after the provider
    /// channel was revoked (live path) or after a restart (restart path, when
    /// the snapshot had no committed terminal), with the original request it
    /// belongs to.
    RecoveredTerminal {
        receipt: LiveBridgeRecoveredTerminalReceipt,
        original: LiveResponsesOriginalRequest,
    },
    /// Restart path, terminal already committed: the durable snapshot re-read
    /// through its owner just before the append, with its committed terminal.
    ValidatedSnapshot {
        snapshot: LiveBridgeRecoverySnapshot,
        terminal: MeerkatExecutionTerminal,
    },
}

impl LiveResponsesOutcomeEvidence {
    pub(crate) fn live_terminal(receipt: LiveBridgeExecutionTerminalReceipt) -> Self {
        Self::LiveTerminal(receipt)
    }

    /// Pair a recovered receipt with its original request. They must name the
    /// same session and exact operation.
    pub(crate) fn recovered_terminal(
        receipt: LiveBridgeRecoveredTerminalReceipt,
        original: LiveResponsesOriginalRequest,
    ) -> Result<Self, LiveResponsesEvidenceError> {
        if receipt.session_id() != original.session_id()
            || receipt.operation() != original.operation()
        {
            return Err(LiveResponsesEvidenceError::MismatchedOriginal);
        }
        Ok(Self::RecoveredTerminal { receipt, original })
    }

    /// A snapshot re-read from its owner. Only a committed terminal is
    /// evidence of an outcome.
    pub(crate) fn validated_snapshot(
        snapshot: LiveBridgeRecoverySnapshot,
    ) -> Result<Self, LiveResponsesEvidenceError> {
        let terminal = snapshot
            .terminal()
            .ok_or(LiveResponsesEvidenceError::UncommittedSnapshot)?;
        Ok(Self::ValidatedSnapshot { snapshot, terminal })
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        match self {
            Self::LiveTerminal(receipt) => receipt.admission().session_id(),
            Self::RecoveredTerminal { receipt, .. } => receipt.session_id(),
            Self::ValidatedSnapshot { snapshot, .. } => snapshot.session_id(),
        }
    }

    pub(crate) fn operation(&self) -> &ExactOperationIdentity<LiveBridgeOperationCorrelation> {
        match self {
            Self::LiveTerminal(receipt) => receipt.admission().operation(),
            Self::RecoveredTerminal { receipt, .. } => receipt.operation(),
            Self::ValidatedSnapshot { snapshot, .. } => snapshot.operation(),
        }
    }

    pub(crate) fn terminal(&self) -> MeerkatExecutionTerminal {
        match self {
            Self::LiveTerminal(receipt) => receipt.terminal(),
            Self::RecoveredTerminal { receipt, .. } => receipt.terminal(),
            Self::ValidatedSnapshot { terminal, .. } => *terminal,
        }
    }

    pub(crate) fn result_digest(&self) -> Option<&str> {
        match self {
            Self::LiveTerminal(receipt) => receipt.result_digest(),
            Self::RecoveredTerminal { receipt, .. } => receipt.result_digest(),
            Self::ValidatedSnapshot { snapshot, .. } => snapshot.result_digest(),
        }
    }

    /// Digest of the original delegated bridge request, not of the append.
    pub(crate) fn original_request_digest(&self) -> &str {
        match self {
            Self::LiveTerminal(receipt) => receipt.admission().request_digest().as_str(),
            Self::RecoveredTerminal { original, .. } => original.request_digest(),
            Self::ValidatedSnapshot { snapshot, .. } => snapshot.request_digest(),
        }
    }

    /// Canonical context revision the original delegated request was made at.
    pub(crate) fn original_canonical_context_revision(&self) -> &str {
        match self {
            Self::LiveTerminal(receipt) => {
                receipt.admission().canonical_context_revision().as_str()
            }
            Self::RecoveredTerminal { original, .. } => original.canonical_context_revision(),
            Self::ValidatedSnapshot { snapshot, .. } => snapshot.canonical_context_revision(),
        }
    }
}

/// Process receipt submitted with one authenticated LiveResponses outcome
/// append. It binds the owner-issued evidence to the runtime owner that issued
/// it (compared as the same live execution owner, not as one wrapper), the
/// receiving session and the exact append request. Only this crate constructs
/// it; it has no serde form. It is a sealed historical outcome, never current
/// permission: the current mandate, source and destination are decided under
/// the actual publication by the owner that composes the verifier.
pub struct LiveResponsesOutcomeReceipt {
    owner: Arc<meerkat_runtime::MeerkatMachine>,
    session_id: SessionId,
    append_request_digest: String,
    evidence: LiveResponsesOutcomeEvidence,
    /// The original work binding the facts were mapped from.
    authority: Arc<meerkat_runtime::live_execution::LiveBridgeOutcomeAuthority>,
    /// The installed owners' targets at issuance.
    source: meerkat_core::SourceAuthorizationTarget,
    destination: meerkat_core::OwnerQualifiedTarget,
}

impl LiveResponsesOutcomeReceipt {
    pub(crate) fn new(
        owner: Arc<meerkat_runtime::MeerkatMachine>,
        request: &meerkat_core::service::AppendSystemContextRequest,
        evidence: LiveResponsesOutcomeEvidence,
        governed: &LiveResponsesGovernedFacts,
    ) -> Result<Self, meerkat_core::OperationAuthorizationError> {
        let append_request_digest =
            meerkat_core::session::context_control::ContextControlAuditRecord::digest_request(
                request,
            )?;
        Ok(Self {
            owner,
            session_id: evidence.session_id().clone(),
            append_request_digest,
            evidence,
            authority: Arc::clone(&governed.authority),
            source: governed.facts.source.clone(),
            destination: governed.facts.destination.clone(),
        })
    }

    /// The original work binding the control facts were mapped from.
    #[must_use]
    pub fn authority(&self) -> &meerkat_runtime::live_execution::LiveBridgeOutcomeAuthority {
        &self.authority
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn operation(&self) -> &ExactOperationIdentity<LiveBridgeOperationCorrelation> {
        self.evidence.operation()
    }

    #[must_use]
    pub fn terminal(&self) -> MeerkatExecutionTerminal {
        self.evidence.terminal()
    }

    #[must_use]
    pub fn result_digest(&self) -> Option<&str> {
        self.evidence.result_digest()
    }

    /// Digest of the original delegated bridge request, not of the append.
    #[must_use]
    pub fn original_request_digest(&self) -> &str {
        self.evidence.original_request_digest()
    }

    #[must_use]
    pub fn original_canonical_context_revision(&self) -> &str {
        self.evidence.original_canonical_context_revision()
    }
}

/// The control facts of one governed outcome append, with the original work
/// binding they were mapped from.
pub(crate) struct LiveResponsesGovernedFacts {
    pub(crate) facts: meerkat_core::service::ContextControlFacts,
    pub(crate) authority: Arc<meerkat_runtime::live_execution::LiveBridgeOutcomeAuthority>,
}

/// Map the control facts of a governed outcome append: requester, actor and
/// realm from the operation's original work binding (its requester, logical
/// executor and ingress realm), source and destination from the installed
/// owners. Any missing binding, owner or mapping is unavailable; nothing is
/// inferred from ids, strings or the current session.
pub(crate) async fn resolve_live_responses_governed_facts(
    runtime: &meerkat_runtime::MeerkatMachine,
    owners: Option<&LiveResponsesOutcomeOwners>,
    session_id: &SessionId,
    operation: &ExactOperationIdentity<LiveBridgeOperationCorrelation>,
) -> Result<LiveResponsesGovernedFacts, super::LiveResponsesOutcomeAppendError> {
    let owners = owners.ok_or(super::LiveResponsesOutcomeAppendError::Unavailable(
        "the outcome source and destination owners are not installed",
    ))?;
    let authority = runtime
        .live_bridge_outcome_authority(session_id, operation)
        .await
        .map_err(|error| {
            super::LiveResponsesOutcomeAppendError::from_binding(
                error,
                "the operation's original work binding is unavailable",
            )
        })?;
    let source = owners.source.outcome_source(session_id, operation)?;
    let destination = owners.destination.session_destination(session_id)?;
    Ok(LiveResponsesGovernedFacts {
        facts: meerkat_core::service::ContextControlFacts {
            requester: authority.requester().clone(),
            actor: authority.logical_executor().clone(),
            realm: authority.realm().clone(),
            source,
            destination,
        },
        authority: Arc::new(authority),
    })
}

fn same_target(
    left: &meerkat_core::OwnerQualifiedTarget,
    right: &meerkat_core::OwnerQualifiedTarget,
) -> bool {
    left.authority == right.authority && left.namespace == right.namespace && left.id == right.id
}

/// A live outcome is a source only under an external owner target.
fn same_outcome_source(
    left: &meerkat_core::SourceAuthorizationTarget,
    right: &meerkat_core::SourceAuthorizationTarget,
) -> bool {
    matches!(
        (left, right),
        (
            meerkat_core::SourceAuthorizationTarget::External(left),
            meerkat_core::SourceAuthorizationTarget::External(right),
        ) if same_target(left, right)
    )
}

/// Installed owner mapping for the outcome of one exact committed bridge
/// operation: which owner target the outcome is a source under. The installed
/// bridge/source owner supplies it; it is never derived from an operation id,
/// an executor, credentials or a source annotation. A missing owner or mapping
/// is typed `Unavailable`.
pub trait LiveResponsesOutcomeSourceOwner: Send + Sync {
    fn outcome_source(
        &self,
        session_id: &SessionId,
        operation: &ExactOperationIdentity<LiveBridgeOperationCorrelation>,
    ) -> Result<meerkat_core::SourceAuthorizationTarget, meerkat_core::OperationAuthorizationError>;
}

/// The receiving session's actual resource-owner target, for the destination
/// and audience of an outcome append. A missing owner or mapping is typed
/// `Unavailable`.
pub trait SessionContextDestinationOwner: Send + Sync {
    fn session_destination(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::OwnerQualifiedTarget, meerkat_core::OperationAuthorizationError>;
}

/// The two owner seams a governed outcome append needs, installed by the host
/// that composes the coordinator. Without them a governed append is typed
/// `Unavailable`.
#[derive(Clone)]
pub struct LiveResponsesOutcomeOwners {
    pub source: Arc<dyn LiveResponsesOutcomeSourceOwner>,
    pub destination: Arc<dyn SessionContextDestinationOwner>,
}

/// Receipt verifier the installed admitted-work owner composes explicitly for
/// LiveResponses outcome appends. It proves only that the control carries this
/// runtime's evidence for this exact append; it grants nothing. The composing
/// owner still resolves the current mandate and source/destination policy.
pub struct LiveResponsesOutcomeVerifier {
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
}

impl LiveResponsesOutcomeVerifier {
    #[must_use]
    pub fn new(runtime: Arc<meerkat_runtime::MeerkatMachine>) -> Self {
        Self { runtime }
    }

    /// Refuse a control unless it carries a LiveResponses receipt issued by
    /// this runtime for the control's session and exact append request, under
    /// the operation-derived outcome identity.
    pub fn verify_context_control<'a>(
        &self,
        control: &'a meerkat_core::service::SystemContextControlRequest,
    ) -> Result<&'a LiveResponsesOutcomeReceipt, meerkat_core::OperationAuthorizationError> {
        let denied = || {
            meerkat_core::OperationAuthorizationError::from(meerkat_core::OperationRefused::new(
                meerkat_core::OperationRefusalKind::Denied,
            ))
        };
        let receipt = control
            .evidence::<LiveResponsesOutcomeReceipt>()
            .ok_or_else(denied)?;
        let operation_id = receipt.operation().operation_id();
        let request = control.request();
        if !receipt.owner.is_same_runtime_owner(&self.runtime)
            || receipt.session_id() != control.session_id()
            || receipt.append_request_digest
                != meerkat_core::session::context_control::ContextControlAuditRecord::digest_request(
                    request,
                )?
            || request.idempotency_key.as_deref()
                != Some(super::responses_outcome_idempotency_key(operation_id).as_str())
            || request.source.as_deref()
                != Some(super::responses_outcome_source(operation_id).as_str())
        {
            return Err(denied());
        }
        // Every control fact must be the one mapped from the original work
        // binding and the installed owners; the receipt cannot authorize a
        // changed requester, actor, realm, source or destination.
        let facts = control.facts();
        if &facts.requester != receipt.authority.requester()
            || &facts.actor != receipt.authority.logical_executor()
            || &facts.realm != receipt.authority.realm()
            || !same_outcome_source(&facts.source, &receipt.source)
            || !same_target(&facts.destination, &receipt.destination)
        {
            return Err(denied());
        }
        Ok(receipt)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::service::{ContextControlFacts, SystemContextControlRequest};
    use meerkat_core::{
        InteractionId, LiveBridgeProviderCorrelation, LiveBridgeRequestDigest, LiveChannelId,
        OperationId, OwnerQualifiedTarget, PrincipalKind, PrincipalRef, SourceAuthorizationTarget,
        TrustDomainId,
    };
    use meerkat_runtime::live_execution::LiveDelegationRuntimeBinding;

    use crate::live_delegation::{
        DurableExecutorTerminalKind, LiveResponsesOutcomeAppendError, LiveResponsesOutcomeRoute,
        live_responses_outcome_profile, responses_outcome_append_request,
        route_live_responses_outcome,
    };

    fn operation() -> ExactOperationIdentity<LiveBridgeOperationCorrelation> {
        let provider =
            LiveBridgeProviderCorrelation::new("provider:turn:1", "delegation:1", "call:1")
                .expect("provider correlation");
        let correlation = LiveBridgeOperationCorrelation::new(
            LiveChannelId::new("live:test"),
            InteractionId::new(),
            provider,
        )
        .expect("bridge correlation");
        ExactOperationIdentity::for_domain(OperationId::new(), correlation)
    }

    fn admission(
        session_id: &SessionId,
        operation: ExactOperationIdentity<LiveBridgeOperationCorrelation>,
    ) -> LiveBridgeOperationAdmission {
        LiveBridgeOperationAdmission::__test_new(
            session_id.clone(),
            LiveDelegationRuntimeBinding::__test_new(
                session_id.clone(),
                LiveChannelId::new("live:test"),
                meerkat_runtime::LogicalRuntimeId::new("runtime:test"),
                1,
                1,
            ),
            operation,
            "test-durable-member",
            meerkat_core::Session::with_id(session_id.clone())
                .canonical_context_revision()
                .expect("context revision"),
            LiveBridgeRequestDigest::derive("check the garden irrigation").expect("digest"),
        )
    }

    fn snapshot(
        session_id: &SessionId,
        operation: ExactOperationIdentity<LiveBridgeOperationCorrelation>,
        terminal: Option<MeerkatExecutionTerminal>,
    ) -> LiveBridgeRecoverySnapshot {
        LiveBridgeRecoverySnapshot::__test_new(
            session_id.clone(),
            operation,
            "revision:original",
            "sha256:original-request",
            terminal,
            Some("sha256:result".to_string()),
        )
    }

    fn principal(value: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            value,
            TrustDomainId::new("live-responses-test").expect("domain"),
        )
        .expect("qualified principal")
    }

    fn facts(session_id: &SessionId) -> ContextControlFacts {
        ContextControlFacts {
            requester: principal("original-requester"),
            actor: principal("live-bridge-producer"),
            realm: meerkat_core::connection::RealmId::parse("live-responses-test").unwrap(),
            source: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: principal("live-bridge-owner"),
                namespace: Arc::from("live-responses-outcome"),
                id: Arc::from("operation"),
            }),
            destination: OwnerQualifiedTarget {
                authority: principal("session-owner"),
                namespace: Arc::from("session-context"),
                id: Arc::from(session_id.to_string()),
            },
        }
    }

    /// Governed facts as the resolver would map them: the `facts` fixture's
    /// requester, actor and realm are the original work binding's.
    fn governed(session_id: &SessionId) -> LiveResponsesGovernedFacts {
        let facts = facts(session_id);
        let identity = meerkat_core::retained_work::RetainedWorkIdentity::new(
            "runtime:test",
            meerkat_core::lifecycle::RunId::new(),
            Vec::new(),
            std::collections::BTreeMap::new(),
            None,
        );
        let authority = meerkat_runtime::live_execution::LiveBridgeOutcomeAuthority::__test_new(
            identity,
            facts.requester.clone(),
            facts.actor.clone(),
            facts.realm.clone(),
        );
        LiveResponsesGovernedFacts {
            facts,
            authority: Arc::new(authority),
        }
    }

    #[test]
    fn live_terminal_evidence_reports_the_admitted_request_and_committed_terminal() {
        let session_id = SessionId::new();
        let admission = admission(&session_id, operation());
        let receipt = LiveBridgeExecutionTerminalReceipt::__test_new(
            admission.clone(),
            MeerkatExecutionTerminal::Completed,
            Some("sha256:result".to_string()),
        );
        let evidence = LiveResponsesOutcomeEvidence::live_terminal(receipt);
        assert_eq!(evidence.session_id(), &session_id);
        assert_eq!(evidence.operation(), admission.operation());
        assert_eq!(evidence.terminal(), MeerkatExecutionTerminal::Completed);
        assert_eq!(evidence.result_digest(), Some("sha256:result"));
        assert_eq!(
            evidence.original_request_digest(),
            admission.request_digest().as_str()
        );
        assert_eq!(
            evidence.original_canonical_context_revision(),
            admission.canonical_context_revision().as_str()
        );
        // The original request's digest is not the digest of the append that
        // carries the outcome.
        let append = responses_outcome_append_request(
            admission.operation().operation_id(),
            DurableExecutorTerminalKind::Completed,
            Some("watered"),
        );
        let append_digest =
            meerkat_core::session::context_control::ContextControlAuditRecord::digest_request(
                &append,
            )
            .expect("append digest");
        assert_ne!(evidence.original_request_digest(), append_digest);
    }

    #[test]
    fn a_recovered_terminal_must_belong_to_its_original_request() {
        let session_id = SessionId::new();
        let original = snapshot(&session_id, operation(), None);
        let foreign = LiveBridgeRecoveredTerminalReceipt::__test_new(
            session_id.clone(),
            operation(),
            MeerkatExecutionTerminal::Completed,
            Some("sha256:result".to_string()),
        );
        assert_eq!(
            LiveResponsesOutcomeEvidence::recovered_terminal(
                foreign,
                LiveResponsesOriginalRequest::Snapshot(original.clone()),
            )
            .unwrap_err(),
            LiveResponsesEvidenceError::MismatchedOriginal
        );
        let other_session = LiveBridgeRecoveredTerminalReceipt::__test_new(
            SessionId::new(),
            original.operation().clone(),
            MeerkatExecutionTerminal::Completed,
            None,
        );
        assert_eq!(
            LiveResponsesOutcomeEvidence::recovered_terminal(
                other_session,
                LiveResponsesOriginalRequest::Snapshot(original.clone()),
            )
            .unwrap_err(),
            LiveResponsesEvidenceError::MismatchedOriginal
        );
        let matching = LiveBridgeRecoveredTerminalReceipt::__test_new(
            session_id.clone(),
            original.operation().clone(),
            MeerkatExecutionTerminal::Failed,
            None,
        );
        let evidence = LiveResponsesOutcomeEvidence::recovered_terminal(
            matching,
            LiveResponsesOriginalRequest::Snapshot(original),
        )
        .expect("matching original");
        assert_eq!(evidence.terminal(), MeerkatExecutionTerminal::Failed);
        assert_eq!(
            evidence.original_request_digest(),
            "sha256:original-request"
        );
        assert_eq!(
            evidence.original_canonical_context_revision(),
            "revision:original"
        );
    }

    #[test]
    fn only_a_committed_snapshot_is_outcome_evidence() {
        let session_id = SessionId::new();
        assert_eq!(
            LiveResponsesOutcomeEvidence::validated_snapshot(snapshot(
                &session_id,
                operation(),
                None
            ))
            .unwrap_err(),
            LiveResponsesEvidenceError::UncommittedSnapshot
        );
        let evidence = LiveResponsesOutcomeEvidence::validated_snapshot(snapshot(
            &session_id,
            operation(),
            Some(MeerkatExecutionTerminal::Completed),
        ))
        .expect("committed snapshot");
        assert_eq!(evidence.terminal(), MeerkatExecutionTerminal::Completed);
        assert_eq!(evidence.result_digest(), Some("sha256:result"));
    }

    /// An installed host that admits nothing: these tests only observe that
    /// one is installed.
    struct RefusingHost;

    impl meerkat_runtime::input_authority::NativeWorkAuthorizationHost for RefusingHost {
        fn authenticate_association(
            &self,
            _runtime_id: &meerkat_runtime::LogicalRuntimeId,
            _input: &meerkat_runtime::input::Input,
            _ingress: &meerkat_runtime::input_authority::NativeIngressContext,
            _association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
        ) -> Result<(), meerkat_runtime::input_authority::NativeAdmissionError> {
            Err(
                meerkat_core::OperationRefused::new(meerkat_core::OperationRefusalKind::Denied)
                    .into(),
            )
        }

        fn work_context(
            &self,
            _batch: &meerkat_runtime::input_authority::NativeWorkBatch,
        ) -> Result<
            meerkat_core::authorization::WorkAuthorizationContext,
            meerkat_runtime::input_authority::NativeWorkContextError,
        > {
            Err(meerkat_runtime::input_authority::NativeWorkContextError::MalformedAcceptedWork)
        }
    }

    fn completed_outcome() -> (
        SessionId,
        meerkat_core::service::AppendSystemContextRequest,
        LiveResponsesOutcomeEvidence,
    ) {
        let session_id = SessionId::new();
        let admission = admission(&session_id, operation());
        let request = responses_outcome_append_request(
            admission.operation().operation_id(),
            DurableExecutorTerminalKind::Completed,
            Some("watered"),
        );
        let evidence = LiveResponsesOutcomeEvidence::live_terminal(
            LiveBridgeExecutionTerminalReceipt::__test_new(
                admission,
                MeerkatExecutionTerminal::Completed,
                Some("sha256:result".to_string()),
            ),
        );
        (session_id, request, evidence)
    }

    /// The profile comes from the operation's recorded binding and the
    /// installed host. A governed operation is never downgraded to a plain
    /// append, and an unbound operation is no authority for a governed one.
    #[test]
    fn the_outcome_profile_follows_the_recorded_binding_and_the_installed_host() {
        assert!(matches!(
            live_responses_outcome_profile(false, false),
            Ok(false)
        ));
        assert!(matches!(
            live_responses_outcome_profile(true, true),
            Ok(true)
        ));
        assert!(matches!(
            live_responses_outcome_profile(true, false),
            Err(LiveResponsesOutcomeAppendError::Unavailable(_))
        ));
        assert!(matches!(
            live_responses_outcome_profile(false, true),
            Err(LiveResponsesOutcomeAppendError::Unavailable(_))
        ));
    }

    #[test]
    fn an_ungoverned_profile_takes_the_plain_append() {
        let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let (session_id, request, evidence) = completed_outcome();
        let route =
            route_live_responses_outcome(&runtime, &session_id, request.clone(), evidence, None)
                .expect("route");
        assert!(matches!(route, LiveResponsesOutcomeRoute::Plain(plain) if plain == request));
    }

    #[test]
    fn with_an_installed_host_the_outcome_is_an_authenticated_control_carrying_its_evidence() {
        let runtime = Arc::new(
            meerkat_runtime::MeerkatMachine::ephemeral()
                .with_native_work_authorization_host(Arc::new(RefusingHost))
                .expect("install host"),
        );
        let (session_id, request, evidence) = completed_outcome();
        let operation = evidence.operation().clone();
        let route = route_live_responses_outcome(
            &runtime,
            &session_id,
            request.clone(),
            evidence,
            Some(governed(&session_id)),
        )
        .expect("route");
        let LiveResponsesOutcomeRoute::Authenticated(control) = route else {
            panic!("an installed host must route the outcome through authenticated append");
        };
        assert_eq!(control.session_id(), &session_id);
        assert_eq!(control.request(), &request);
        let verified = LiveResponsesOutcomeVerifier::new(Arc::clone(&runtime))
            .verify_context_control(&control)
            .expect("the control carries this runtime's receipt");
        assert_eq!(verified.operation(), &operation);
    }

    #[tokio::test]
    async fn a_fresh_runtime_over_the_same_store_is_not_the_issuing_owner() {
        let store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let issuing = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("issuing runtime"),
        );
        let fresh = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("fresh runtime over the same store"),
        );
        assert!(issuing.shares_runtime_store_authority(&store));
        assert!(fresh.shares_runtime_store_authority(&store));
        let (session_id, request, evidence) = completed_outcome();
        let receipt = Arc::new(
            LiveResponsesOutcomeReceipt::new(
                Arc::clone(&issuing),
                &request,
                evidence,
                &governed(&session_id),
            )
            .expect("receipt"),
        );
        let control = SystemContextControlRequest::from_trusted_ingress(
            session_id.clone(),
            request,
            facts(&session_id),
            receipt,
        )
        .expect("control");
        assert!(
            LiveResponsesOutcomeVerifier::new(Arc::clone(&issuing))
                .verify_context_control(&control)
                .is_ok()
        );
        assert!(
            LiveResponsesOutcomeVerifier::new(Arc::new((*issuing).clone()))
                .verify_context_control(&control)
                .is_ok()
        );
        assert!(
            LiveResponsesOutcomeVerifier::new(fresh)
                .verify_context_control(&control)
                .is_err(),
            "sharing a store is not sharing the runtime owner"
        );
    }

    /// A governed service's refusal as it reaches the producer: inside a
    /// `SessionControlError`, not built directly as a producer error.
    fn service_refusal(kind: usize) -> meerkat_core::service::SessionControlError {
        use meerkat_core::OperationAuthorizationError as Native;
        meerkat_core::service::SessionControlError::Authorization(match kind {
            0 => Native::Refused(meerkat_core::OperationRefused::new(
                meerkat_core::OperationRefusalKind::Denied,
            )),
            1 => Native::Unavailable,
            _ => Native::ObservationUnavailable(
                meerkat_core::authorization::OperationObservationError,
            ),
        })
    }

    fn applied() -> meerkat_core::AppendSystemContextResult {
        meerkat_core::AppendSystemContextResult {
            status: meerkat_core::AppendSystemContextStatus::Applied,
        }
    }

    /// Evidence limit: the refusals are injected through the projection
    /// helper's append closure as the service would return them; no
    /// authenticated session service is called.
    #[tokio::test(start_paused = true)]
    async fn service_refusals_settle_once_without_marking_the_outcome_projected() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::live_delegation::{
            LiveResponsesOutcomeProjection, project_live_responses_outcome,
        };

        let operation_id = OperationId::new();
        let shutdown = tokio_util::sync::CancellationToken::new();
        for kind in 0..3 {
            let appends = AtomicUsize::new(0);
            let receipts = AtomicUsize::new(0);
            let outcome = project_live_responses_outcome(
                &operation_id,
                &shutdown,
                || {
                    appends.fetch_add(1, Ordering::SeqCst);
                    let error = LiveResponsesOutcomeAppendError::from(service_refusal(kind));
                    async move { Err(error) }
                },
                || {
                    receipts.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                },
            )
            .await;
            assert!(
                matches!(
                    outcome,
                    LiveResponsesOutcomeProjection::Settled(
                        LiveResponsesOutcomeAppendError::Authorization(_)
                    )
                ),
                "refusal {kind} keeps its native authorization class: {outcome:?}"
            );
            assert_eq!(
                appends.load(Ordering::SeqCst),
                1,
                "refusal {kind} is not retried"
            );
            assert_eq!(
                receipts.load(Ordering::SeqCst),
                0,
                "refusal {kind} never marks the outcome projected"
            );
        }

        // A deleted destination settles once too.
        let appends = AtomicUsize::new(0);
        let receipts = AtomicUsize::new(0);
        let outcome = project_live_responses_outcome(
            &operation_id,
            &shutdown,
            || {
                appends.fetch_add(1, Ordering::SeqCst);
                let error = LiveResponsesOutcomeAppendError::from(
                    meerkat_core::service::SessionControlError::Session(
                        meerkat_core::service::SessionError::NotFound {
                            id: SessionId::new(),
                        },
                    ),
                );
                async move { Err(error) }
            },
            || {
                receipts.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
        )
        .await;
        assert!(matches!(
            outcome,
            LiveResponsesOutcomeProjection::Settled(LiveResponsesOutcomeAppendError::Rejected(_))
        ));
        assert_eq!(appends.load(Ordering::SeqCst), 1);
        assert_eq!(receipts.load(Ordering::SeqCst), 0);

        // Independent permitted work continues: a transient busy session is
        // retried, then the append applies and the outcome receipt is recorded.
        let appends = AtomicUsize::new(0);
        let receipts = AtomicUsize::new(0);
        let outcome = project_live_responses_outcome(
            &OperationId::new(),
            &shutdown,
            || {
                let attempt = appends.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt == 0 {
                        Err(LiveResponsesOutcomeAppendError::from(
                            meerkat_core::service::SessionControlError::Session(
                                meerkat_core::service::SessionError::Busy {
                                    id: SessionId::new(),
                                },
                            ),
                        ))
                    } else {
                        Ok(applied())
                    }
                }
            },
            || {
                receipts.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
        )
        .await;
        assert!(matches!(outcome, LiveResponsesOutcomeProjection::Projected));
        assert_eq!(appends.load(Ordering::SeqCst), 2);
        assert_eq!(receipts.load(Ordering::SeqCst), 1);
    }

    /// Evidence limit: the binding failures are injected through the
    /// projection helper's append closure, mapped exactly as the real path
    /// maps the runtime's binding errors; no runtime store fails here.
    #[tokio::test(start_paused = true)]
    async fn transient_binding_failures_are_retried_and_absent_bindings_settle_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use meerkat_runtime::live_execution::LiveBridgeOutcomeBindingError as Binding;

        use crate::live_delegation::{
            LiveResponsesOutcomeProjection, project_live_responses_outcome,
        };

        let shutdown = tokio_util::sync::CancellationToken::new();
        let transient_causes = [
            meerkat_runtime::RuntimeDriverError::NotReady {
                state: meerkat_runtime::RuntimeState::Destroyed,
            },
            meerkat_runtime::RuntimeDriverError::Internal("retained row read failed".into()),
        ];
        for cause in transient_causes {
            let cause = std::sync::Mutex::new(Some(cause));
            let appends = AtomicUsize::new(0);
            let receipts = AtomicUsize::new(0);
            let outcome = project_live_responses_outcome(
                &OperationId::new(),
                &shutdown,
                || {
                    appends.fetch_add(1, Ordering::SeqCst);
                    let first = cause.lock().unwrap().take();
                    async move {
                        match first {
                            Some(cause) => Err(LiveResponsesOutcomeAppendError::from_binding(
                                Binding::from(cause),
                                "absent",
                            )),
                            None => Ok(applied()),
                        }
                    }
                },
                || {
                    receipts.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                },
            )
            .await;
            assert!(
                matches!(outcome, LiveResponsesOutcomeProjection::Projected),
                "a transient binding failure is retried, never settled: {outcome:?}"
            );
            assert_eq!(appends.load(Ordering::SeqCst), 2);
            assert_eq!(receipts.load(Ordering::SeqCst), 1);
        }

        // An absent or malformed binding settles once, unmarked.
        for cause in [
            meerkat_runtime::RuntimeDriverError::ValidationFailed {
                reason: "no recorded binding".into(),
            },
            meerkat_runtime::RuntimeDriverError::ValidationFailed {
                reason: "malformed binding".into(),
            },
        ] {
            let cause = std::sync::Mutex::new(Some(cause));
            let appends = AtomicUsize::new(0);
            let receipts = AtomicUsize::new(0);
            let outcome = project_live_responses_outcome(
                &OperationId::new(),
                &shutdown,
                || {
                    appends.fetch_add(1, Ordering::SeqCst);
                    let error = LiveResponsesOutcomeAppendError::from_binding(
                        Binding::from(cause.lock().unwrap().take().expect("one attempt")),
                        "absent",
                    );
                    async move { Err(error) }
                },
                || {
                    receipts.fetch_add(1, Ordering::SeqCst);
                    async { Ok(()) }
                },
            )
            .await;
            assert!(matches!(
                outcome,
                LiveResponsesOutcomeProjection::Settled(
                    LiveResponsesOutcomeAppendError::Unavailable(_)
                )
            ));
            assert_eq!(appends.load(Ordering::SeqCst), 1);
            assert_eq!(receipts.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn the_verifier_accepts_only_this_runtimes_receipt_for_the_exact_append() {
        let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let admission = admission(&session_id, operation());
        let operation_id = admission.operation().operation_id().clone();
        let evidence = LiveResponsesOutcomeEvidence::live_terminal(
            LiveBridgeExecutionTerminalReceipt::__test_new(
                admission,
                MeerkatExecutionTerminal::Completed,
                Some("sha256:result".to_string()),
            ),
        );
        let request = responses_outcome_append_request(
            &operation_id,
            DurableExecutorTerminalKind::Completed,
            Some("watered"),
        );
        let receipt = Arc::new(
            LiveResponsesOutcomeReceipt::new(
                Arc::clone(&runtime),
                &request,
                evidence.clone(),
                &governed(&session_id),
            )
            .expect("receipt"),
        );
        let control = SystemContextControlRequest::from_trusted_ingress(
            session_id.clone(),
            request.clone(),
            facts(&session_id),
            Arc::clone(&receipt),
        )
        .expect("control");
        let verifier = LiveResponsesOutcomeVerifier::new(Arc::clone(&runtime));
        let verified = verifier
            .verify_context_control(&control)
            .expect("this runtime's receipt for this exact append");
        assert_eq!(verified.operation().operation_id(), &operation_id);
        assert_eq!(verified.terminal(), MeerkatExecutionTerminal::Completed);

        // A distinct wrapper around the same runtime owner is the same owner.
        let same_owner = Arc::new((*runtime).clone());
        assert!(!Arc::ptr_eq(&same_owner, &runtime));
        assert!(
            LiveResponsesOutcomeVerifier::new(same_owner)
                .verify_context_control(&control)
                .is_ok()
        );

        // Another runtime owner did not issue it.
        let other_runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        assert!(
            LiveResponsesOutcomeVerifier::new(other_runtime)
                .verify_context_control(&control)
                .is_err()
        );

        // A different append request under the same receipt.
        let mut changed = request.clone();
        changed.content =
            meerkat_core::lifecycle::run_primitive::CoreRenderable::text("something else");
        let changed_control = SystemContextControlRequest::from_trusted_ingress(
            session_id.clone(),
            changed,
            facts(&session_id),
            Arc::clone(&receipt),
        )
        .expect("control");
        assert!(verifier.verify_context_control(&changed_control).is_err());

        // The same receipt submitted to another session.
        let elsewhere = SessionId::new();
        let elsewhere_control = SystemContextControlRequest::from_trusted_ingress(
            elsewhere.clone(),
            request.clone(),
            facts(&elsewhere),
            Arc::clone(&receipt),
        )
        .expect("control");
        assert!(verifier.verify_context_control(&elsewhere_control).is_err());

        // A request without the operation's outcome identity.
        let mut unkeyed = request.clone();
        unkeyed.idempotency_key = Some("some-other-key".to_string());
        let unkeyed_receipt = Arc::new(
            LiveResponsesOutcomeReceipt::new(
                Arc::clone(&runtime),
                &unkeyed,
                evidence,
                &governed(&session_id),
            )
            .expect("receipt"),
        );
        let unkeyed_control = SystemContextControlRequest::from_trusted_ingress(
            session_id.clone(),
            unkeyed,
            facts(&session_id),
            unkeyed_receipt,
        )
        .expect("control");
        assert!(verifier.verify_context_control(&unkeyed_control).is_err());

        // A control whose facts differ from the original work binding's.
        let mut changed_facts = facts(&session_id);
        changed_facts.requester = principal("someone-else");
        let changed_facts_control = SystemContextControlRequest::from_trusted_ingress(
            session_id.clone(),
            request.clone(),
            changed_facts,
            Arc::clone(&receipt),
        )
        .expect("control");
        assert!(
            verifier
                .verify_context_control(&changed_facts_control)
                .is_err()
        );
        let mut changed_destination = facts(&session_id);
        changed_destination.destination.id = Arc::from("another-session");
        let changed_destination_control = SystemContextControlRequest::from_trusted_ingress(
            session_id.clone(),
            request.clone(),
            changed_destination,
            Arc::clone(&receipt),
        )
        .expect("control");
        assert!(
            verifier
                .verify_context_control(&changed_destination_control)
                .is_err()
        );

        // Evidence of any other type is not LiveResponses evidence.
        let foreign_control = SystemContextControlRequest::from_trusted_ingress(
            session_id.clone(),
            request,
            facts(&session_id),
            Arc::new(42_u8),
        )
        .expect("control");
        assert!(verifier.verify_context_control(&foreign_control).is_err());
    }
}
